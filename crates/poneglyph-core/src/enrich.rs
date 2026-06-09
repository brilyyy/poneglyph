//! Background enrichment queue (PRD §8.4 / §8.11 groundwork).
//!
//! M3 scope: a persistent `jobs` table plus a tokio worker that drains
//! `compute_edges` jobs. Jobs are enqueued transactionally next to the
//! memory write and survive crashes; the mpsc channel is only a wake-up
//! signal, never the source of truth. LLM job types are recorded but not
//! executed until M6 — they are marked failed instead of looping forever.
//!
//! Edge computation must never block `remember`: callers insert a job row
//! (fast) and `notify()` the worker (non-blocking try_send).

use std::path::PathBuf;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::GraphConfig;
use crate::graph;
use crate::model::{JobStatus, JobType};
use crate::store::Store;

/// Max jobs drained per pass.
const DRAIN_BATCH: usize = 64;

/// Fallback poll interval so jobs enqueued by other processes (e.g. the CLI
/// while `serve` runs) are picked up even without a notify.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Enqueue + drain (shared by worker and one-shot CLI)
// ---------------------------------------------------------------------------

/// Insert a `compute_edges` job for a memory. Cheap; call inline on remember.
pub fn enqueue_compute_edges(store: &Store, memory_id: &str) -> Result<()> {
    store.create_job(JobType::ComputeEdges, memory_id)?;
    Ok(())
}

/// Drain up to `DRAIN_BATCH` pending jobs once. Returns jobs processed.
/// Individual job failures are recorded on the job row, never propagated —
/// a bad job must not take down the worker.
pub fn process_pending_jobs(store: &Store, graph_cfg: &GraphConfig) -> Result<usize> {
    let jobs = store.get_pending_jobs(DRAIN_BATCH)?;
    let mut processed = 0;

    for job in jobs {
        store.update_job_status(&job.id, JobStatus::Running, None)?;

        let outcome = match job.job_type {
            JobType::ComputeEdges => {
                graph::build_edges_for_memory(store, graph_cfg, &job.memory_id).map(|n| {
                    debug!(memory_id = %job.memory_id, edges = n, "computed edges");
                })
            }
            // LLM enrichment lands in M6; don't leave these pending forever.
            other => Err(anyhow::anyhow!("job type {other} not implemented until M6")),
        };

        match outcome {
            Ok(()) => store.update_job_status(&job.id, JobStatus::Done, None)?,
            Err(e) => {
                warn!(job_id = %job.id, error = %e, "job failed");
                store.update_job_status(&job.id, JobStatus::Failed, Some(&e.to_string()))?;
            }
        }
        processed += 1;
    }

    Ok(processed)
}

// ---------------------------------------------------------------------------
// Background worker
// ---------------------------------------------------------------------------

/// Clonable wake-up handle for the background worker.
#[derive(Clone, Debug)]
pub struct EnrichHandle {
    tx: mpsc::Sender<()>,
}

impl EnrichHandle {
    /// Non-blocking nudge; a full channel is fine (worker is already awake).
    pub fn notify(&self) {
        let _ = self.tx.try_send(());
    }
}

/// Spawn the enrichment worker on its own DB connection (WAL allows this to
/// run alongside the server connection). Returns the wake-up handle and the
/// task handle; the worker exits after all handles are dropped and the queue
/// is drained.
pub fn spawn_worker(
    db_path: PathBuf,
    graph_cfg: GraphConfig,
) -> (EnrichHandle, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<()>(16);
    let handle = EnrichHandle { tx };

    let task = tokio::spawn(async move {
        let store = match Store::open(&db_path) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "enrich worker failed to open store; edges disabled");
                return;
            }
        };
        info!("enrichment worker started");

        // Store isn't Sync, so shuttle it through spawn_blocking by value.
        let mut store = Some(store);
        let mut open = true;
        while open {
            let s = store.take().expect("store always returned");
            let cfg = graph_cfg.clone();
            let result = tokio::task::spawn_blocking(move || {
                let drained = process_pending_jobs(&s, &cfg);
                (s, drained)
            })
            .await;

            match result {
                Ok((s, drained)) => {
                    if let Err(e) = drained {
                        warn!(error = %e, "drain pass failed");
                    }
                    store = Some(s);
                }
                Err(e) => {
                    warn!(error = %e, "drain task panicked; worker stopping");
                    return;
                }
            }

            // Sleep until a notify, the poll interval, or channel close.
            tokio::select! {
                msg = rx.recv() => {
                    if msg.is_none() {
                        open = false; // all senders dropped — final drain done above
                    }
                }
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
        info!("enrichment worker stopped");
    });

    (handle, task)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MemoryType, Source};

    #[test]
    fn enqueue_and_drain_compute_edges() {
        let store = Store::open_in_memory().unwrap();
        let cfg = GraphConfig::default();
        let p = store.upsert_project("/p", "p", None).unwrap();

        let m1 = store
            .create_memory("first", MemoryType::Fact, 0.5, Source::Cli, Some(&p.id), None)
            .unwrap();
        let m2 = store
            .create_memory("second", MemoryType::Fact, 0.5, Source::Cli, Some(&p.id), None)
            .unwrap();

        enqueue_compute_edges(&store, &m2.id).unwrap();
        assert_eq!(store.stats().unwrap().pending_jobs, 1);

        let processed = process_pending_jobs(&store, &cfg).unwrap();
        assert_eq!(processed, 1);
        assert_eq!(store.stats().unwrap().pending_jobs, 0);

        // Temporal edge m1<->m2 (same project, same instant).
        let edges = store.get_edges_for_memory(&m1.id).unwrap();
        assert_eq!(edges.len(), 1);

        // Re-enqueue + re-drain: idempotent (unique edge constraint).
        enqueue_compute_edges(&store, &m2.id).unwrap();
        process_pending_jobs(&store, &cfg).unwrap();
        assert_eq!(store.get_edges_for_memory(&m1.id).unwrap().len(), 1);
        let _ = m2;
    }

    #[test]
    fn llm_jobs_fail_gracefully_until_m6() {
        let store = Store::open_in_memory().unwrap();
        let cfg = GraphConfig::default();
        let m = store
            .create_memory("x", MemoryType::Fact, 0.5, Source::Cli, None, None)
            .unwrap();

        store.create_job(JobType::Summarize, &m.id).unwrap();
        process_pending_jobs(&store, &cfg).unwrap();

        let status: String = store
            .conn
            .query_row("SELECT status FROM jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "failed");
    }

    #[tokio::test]
    async fn worker_drains_on_notify() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("w.db");

        let store = Store::open(&db_path).unwrap();
        let p = store.upsert_project("/p", "p", None).unwrap();
        let m1 = store
            .create_memory("a", MemoryType::Fact, 0.5, Source::Cli, Some(&p.id), None)
            .unwrap();
        let _m2 = store
            .create_memory("b", MemoryType::Fact, 0.5, Source::Cli, Some(&p.id), None)
            .unwrap();
        enqueue_compute_edges(&store, &m1.id).unwrap();

        let (handle, task) = spawn_worker(db_path.clone(), GraphConfig::default());
        handle.notify();

        // Poll until the worker has drained the job (bounded wait).
        let mut done = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if store.stats().unwrap().pending_jobs == 0 {
                done = true;
                break;
            }
        }
        assert!(done, "worker should drain the queue");
        assert_eq!(store.get_edges_for_memory(&m1.id).unwrap().len(), 1);

        drop(handle);
        task.await.unwrap();
    }
}
