//! Background enrichment queue (PRD §8.4 / §8.11).
//!
//! The persistent `jobs` table is the source of truth; the mpsc channel is
//! only a wake-up signal. Jobs are enqueued transactionally next to the
//! memory write and survive crashes. Edge computation must never block
//! `remember`: callers insert a job row (fast) and `notify()` (non-blocking).
//!
//! Retry model: `attempts` counts executions started (bumped only by
//! `mark_job_running`). A failed job goes back to `pending` with
//! `updated_at` as the retry timestamp; the drain loop skips jobs still in
//! their backoff window (10s · 2^attempts). After `MAX_ATTEMPTS` it is
//! marked `failed`. Individual job failures never take down the worker.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::{CompressionMode, Config, EnrichmentConfig, LlmConfig, MemoryEdgesConfig};
use crate::embed::Embedder;
use crate::graph;
use crate::model::{Job, JobStatus, JobType};
use crate::store::Store;

/// Max jobs drained per pass.
const DRAIN_BATCH: usize = 64;

/// Executions before a job is marked failed for good.
const MAX_ATTEMPTS: i64 = 3;

/// Fallback poll interval so jobs enqueued by other processes (e.g. the CLI
/// while `serve` runs) — and retry-backoff expiries — are picked up even
/// without a notify.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Everything the background worker needs to know.
#[derive(Clone)]
pub struct WorkerConfig {
    pub edges: MemoryEdgesConfig,
    pub llm: LlmConfig,
    pub enrichment: EnrichmentConfig,
    /// Compression is orthogonal to `enrichment.enabled` — semantic mode
    /// still needs an LLM client, so the worker must know to construct one
    /// even when enrichment itself is off.
    pub compression_enabled: bool,
    pub compression_mode: CompressionMode,
    /// Consolidation scheduler: runs the raw→episodic→semantic→procedural
    /// pipeline + decay across all projects every `interval_hours`. `None`
    /// disables the scheduler tick entirely.
    pub consolidation: Option<ConsolidationScheduler>,
    /// Live-status registry: when set, the worker marks "enrich" active while
    /// draining a non-empty batch and "consolidate" active during a scheduled
    /// pass, so the viewer's activity panel can see them. `None` ⇒ no tracking.
    pub activity: Option<Arc<crate::activity::Activity>>,
}

/// `pipeline::run_pipeline_for_all_projects` and `consolidate::run_decay`
/// both take the full `Config` (they read `decay`, `consolidation`,
/// `embedding`, `llm`...), unlike the flattened fields above which only
/// cover job draining — so the scheduler carries its own full config copy
/// rather than re-flattening everything those two functions touch.
#[derive(Clone)]
pub struct ConsolidationScheduler {
    pub config: Config,
    pub embedder: Option<Arc<Embedder>>,
}

// ---------------------------------------------------------------------------
// Enqueue + retry plumbing
// ---------------------------------------------------------------------------

/// Insert a `compute_edges` job for a memory. Cheap; call inline on remember.
pub fn enqueue_compute_edges(store: &Store, memory_id: &str) -> Result<()> {
    store.create_job(JobType::ComputeEdges, memory_id)?;
    Ok(())
}

/// Enqueue the four LLM enrichment jobs for a memory (caller gates on config).
pub fn enqueue_llm_jobs(store: &Store, memory_id: &str) -> Result<()> {
    for jt in [
        JobType::Summarize,
        JobType::ExtractEntities,
        JobType::ExtractRelations,
        JobType::ScoreImportance,
    ] {
        store.create_job(jt, memory_id)?;
    }
    Ok(())
}

/// Compress a memory for context injection per `[memory].compression_mode`
/// (caller gates on `compression_enabled`). Caveman mode is byte-exact and
/// cheap enough to run inline, synchronously, right here — no job needed.
/// Semantic mode needs the async LLM client, so it goes through the job
/// queue like the other LLM jobs; the worker falls back to caveman if no
/// LLM ends up configured (see `llm::compress_caveman_fallback`).
pub fn enqueue_compression(store: &Store, memory_id: &str, mode: CompressionMode) -> Result<()> {
    match mode {
        CompressionMode::Caveman => {
            if let Some(m) = store.get_memory(memory_id)?
                && m.content.len() >= crate::llm::SUMMARIZE_MIN_CHARS
            {
                let compressed = crate::compress::compress(&m.content);
                store.set_compressed_content(memory_id, &compressed, "caveman")?;
            }
            Ok(())
        }
        CompressionMode::Semantic => {
            store.create_job(JobType::ExtractCompress, memory_id)?;
            Ok(())
        }
    }
}

fn backoff(attempts: i64) -> chrono::Duration {
    chrono::Duration::seconds(10 * (1 << attempts.clamp(0, 6)))
}

/// A pending job is due when it has never run, or its backoff has expired.
fn job_due(job: &Job, now: chrono::DateTime<Utc>) -> bool {
    job.attempts == 0 || now >= job.updated_at + backoff(job.attempts)
}

/// Record a failure: back to pending (retry) below the cap, failed at it.
/// `job.attempts` here is the pre-claim value; the claim added one.
fn fail_or_retry(store: &Store, job: &Job, err: &anyhow::Error) -> Result<()> {
    let executed = job.attempts + 1;
    if executed >= MAX_ATTEMPTS {
        warn!(job_id = %job.id, attempts = executed, error = %err, "job failed permanently");
        store.update_job_status(&job.id, JobStatus::Failed, Some(&err.to_string()))
    } else {
        debug!(job_id = %job.id, attempts = executed, error = %err, "job failed; will retry");
        store.update_job_status(&job.id, JobStatus::Pending, Some(&err.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Sync drain — CLI one-shot path (edges only)
// ---------------------------------------------------------------------------

/// Drain due `compute_edges` jobs once; LLM jobs are left pending for the
/// resident `serve` worker (the jobs table is the source of truth).
/// Returns jobs processed.
pub fn process_pending_jobs(store: &Store, edges_cfg: &MemoryEdgesConfig) -> Result<usize> {
    let now = Utc::now();
    let jobs = store.get_pending_jobs(DRAIN_BATCH)?;
    let mut processed = 0;

    for job in jobs {
        if job.job_type != JobType::ComputeEdges || !job_due(&job, now) {
            continue;
        }
        store.mark_job_running(&job.id)?;
        if !edges_cfg.enabled {
            store.update_job_status(&job.id, JobStatus::Done, None)?;
            processed += 1;
            continue;
        }
        match graph::build_edges_for_memory(store, edges_cfg, &job.memory_id) {
            Ok(n) => {
                debug!(memory_id = %job.memory_id, edges = n, "computed edges");
                store.update_job_status(&job.id, JobStatus::Done, None)?;
            }
            Err(e) => fail_or_retry(store, &job, &e)?,
        }
        processed += 1;
    }

    Ok(processed)
}

// ---------------------------------------------------------------------------
// Async drain — resident worker (edges + LLM)
// ---------------------------------------------------------------------------

/// One async drain pass. Edge jobs run inline (ms-scale sqlite work); LLM
/// jobs await the client. Returns jobs processed.
///
/// Takes `&mut Store` even though no mutation is needed: `&mut Store` is
/// `Send` (Store is `Send` but not `Sync`), which lets this future be held
/// across awaits inside `tokio::spawn`.
pub async fn process_jobs_async(
    store: &mut Store,
    cfg: &WorkerConfig,
    llm: Option<&crate::llm::LlmClient>,
) -> Result<usize> {
    let now = Utc::now();
    let jobs = store.get_pending_jobs(DRAIN_BATCH)?;
    // Mark "enrich" active only when there's a batch to drain — an empty pass
    // is microseconds and shouldn't light up the panel.
    let _activity = cfg.activity.as_ref().filter(|_| !jobs.is_empty()).map(|a| a.begin("enrich"));
    let mut processed = 0;

    for job in jobs {
        if !job_due(&job, now) {
            continue;
        }
        store.mark_job_running(&job.id)?;

        let outcome: Result<()> = match job.job_type {
            JobType::ComputeEdges if !cfg.edges.enabled => Ok(()),
            JobType::ComputeEdges => {
                graph::build_edges_for_memory(&*store, &cfg.edges, &job.memory_id).map(|n| {
                    debug!(memory_id = %job.memory_id, edges = n, "computed edges");
                })
            }
            // Entities/relations only feed the graph — no graph, no point.
            JobType::ExtractEntities | JobType::ExtractRelations if !cfg.edges.enabled => Ok(()),
            // Compression degrades gracefully instead of failing: no LLM
            // configured just means the caveman fallback runs in its place.
            JobType::ExtractCompress => match llm {
                Some(client) => {
                    crate::llm::run_job(&mut *store, client, &job.job_type, &job.memory_id).await
                }
                None => crate::llm::compress_caveman_fallback(&mut *store, &job.memory_id),
            },
            ref llm_type => match llm {
                Some(client) => crate::llm::run_job(&mut *store, client, llm_type, &job.memory_id).await,
                // Stale rows from a previously-enabled config: fail once,
                // no point retrying while disabled.
                None => {
                    store.update_job_status(
                        &job.id,
                        JobStatus::Failed,
                        Some("LLM enrichment is disabled"),
                    )?;
                    processed += 1;
                    continue;
                }
            },
        };

        match outcome {
            Ok(()) => store.update_job_status(&job.id, JobStatus::Done, None)?,
            Err(e) => fail_or_retry(store, &job, &e)?,
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
/// run alongside the server connection). `Store` is `Send` (not `Sync`), so
/// the worker task owns it outright and may hold it across awaits — no mutex
/// involved. The LLM client is constructed once, and only when enrichment is
/// enabled (PRD §8.11 AC1). The worker exits after all handles are dropped.
pub fn spawn_worker(
    db_path: PathBuf,
    cfg: WorkerConfig,
) -> (EnrichHandle, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<()>(16);
    let handle = EnrichHandle { tx };

    let task = tokio::spawn(async move {
        let mut store = match Store::open(&db_path) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "enrich worker failed to open store; enrichment disabled");
                return;
            }
        };

        // Compression is orthogonal to enrichment: semantic mode needs an
        // LLM client even when enrichment.enabled is false.
        let needs_llm = cfg.enrichment.enabled
            || (cfg.compression_enabled && cfg.compression_mode == CompressionMode::Semantic);
        let llm = if needs_llm {
            match crate::llm::LlmClient::from_config(&cfg.llm) {
                Some(c) => {
                    info!(model = %cfg.llm.model.as_deref().unwrap_or("?"), "LLM client constructed");
                    Some(c)
                }
                None => {
                    warn!("enrichment/compression enabled but llm config incomplete (need enabled+endpoint+model); LLM jobs will fail, semantic compression will fall back to caveman");
                    None
                }
            }
        } else {
            None
        };

        // ponytail: coarse interval timer, not cron — fine for a single
        // resident worker; upgrade if multi-instance scheduling matters.
        let mut last_consolidation = tokio::time::Instant::now();

        info!("enrichment worker started");
        loop {
            if let Err(e) = process_jobs_async(&mut store, &cfg, llm.as_ref()).await {
                warn!(error = %e, "drain pass failed");
            }

            if let Some(scheduler) = &cfg.consolidation {
                let interval = std::time::Duration::from_secs(scheduler.config.consolidation.interval_hours.max(1) * 3600);
                if scheduler.config.consolidation.enabled && last_consolidation.elapsed() >= interval {
                    last_consolidation = tokio::time::Instant::now();
                    let _activity = cfg.activity.as_ref().map(|a| a.begin("consolidate"));
                    match crate::pipeline::run_pipeline_for_all_projects(
                        &mut store,
                        &scheduler.config,
                        scheduler.embedder.as_deref(),
                        llm.as_ref(),
                    )
                    .await
                    {
                        Ok(report) => info!(?report, "scheduled consolidation pipeline complete"),
                        Err(e) => warn!(error = %e, "scheduled consolidation pipeline failed"),
                    }
                    if let Err(e) = crate::consolidate::run_decay(&store, &scheduler.config) {
                        warn!(error = %e, "scheduled decay run failed");
                    }
                    if let Err(e) = store.mark_consolidation_run() {
                        warn!(error = %e, "failed to record consolidation run timestamp");
                    }
                }
            }

            // Sleep until a notify, the poll interval, or channel close.
            tokio::select! {
                msg = rx.recv() => {
                    if msg.is_none() {
                        break; // all senders dropped — final drain done above
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

    fn worker_cfg() -> WorkerConfig {
        WorkerConfig {
            edges: MemoryEdgesConfig::default(),
            llm: LlmConfig::default(),
            enrichment: EnrichmentConfig::default(),
            compression_enabled: false,
            compression_mode: CompressionMode::default(),
            consolidation: None,
            activity: None,
        }
    }

    #[test]
    fn enqueue_and_drain_compute_edges() {
        let store = Store::open_in_memory().unwrap();
        let cfg = MemoryEdgesConfig::default();
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
    fn process_pending_jobs_noops_compute_edges_when_disabled() {
        let store = Store::open_in_memory().unwrap();
        let cfg = MemoryEdgesConfig { enabled: false, ..MemoryEdgesConfig::default() };
        let p = store.upsert_project("/p", "p", None).unwrap();

        let m1 = store
            .create_memory("first", MemoryType::Fact, 0.5, Source::Cli, Some(&p.id), None)
            .unwrap();
        let m2 = store
            .create_memory("second", MemoryType::Fact, 0.5, Source::Cli, Some(&p.id), None)
            .unwrap();

        enqueue_compute_edges(&store, &m2.id).unwrap();
        let processed = process_pending_jobs(&store, &cfg).unwrap();
        assert_eq!(processed, 1, "job is drained (marked Done), just skipped");
        assert_eq!(store.stats().unwrap().pending_jobs, 0);
        assert_eq!(store.get_edges_for_memory(&m1.id).unwrap().len(), 0, "no edges built while disabled");
    }

    #[test]
    fn sync_drain_skips_llm_jobs() {
        let store = Store::open_in_memory().unwrap();
        let cfg = MemoryEdgesConfig::default();
        let m = store
            .create_memory("x", MemoryType::Fact, 0.5, Source::Cli, None, None)
            .unwrap();

        store.create_job(JobType::Summarize, &m.id).unwrap();
        let processed = process_pending_jobs(&store, &cfg).unwrap();
        assert_eq!(processed, 0);

        // Stays pending for the resident worker — jobs table is the truth.
        let status: String = store
            .conn
            .query_row("SELECT status FROM jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "pending");
    }

    #[tokio::test]
    async fn async_drain_fails_llm_jobs_when_disabled() {
        let mut store = Store::open_in_memory().unwrap();
        let m = store
            .create_memory("x", MemoryType::Fact, 0.5, Source::Cli, None, None)
            .unwrap();
        store.create_job(JobType::Summarize, &m.id).unwrap();

        process_jobs_async(&mut store, &worker_cfg(), None).await.unwrap();

        let status: String = store
            .conn
            .query_row("SELECT status FROM jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "failed");
    }

    #[test]
    fn attempts_count_executions_and_backoff_gates_retry() {
        let store = Store::open_in_memory().unwrap();
        // Job referencing a memory that doesn't exist → edge builder error?
        // build_edges_for_memory on a missing memory returns Ok(0) — use an
        // artificial failure instead: drop the memory after enqueue so the
        // FK cascade removes the job; simpler: drive fail_or_retry directly.
        let m = store
            .create_memory("x", MemoryType::Fact, 0.5, Source::Cli, None, None)
            .unwrap();
        let job = store.create_job(JobType::Summarize, &m.id).unwrap();

        let err = anyhow::anyhow!("boom");

        // Execution 1: claim + fail → pending (retry).
        store.mark_job_running(&job.id).unwrap();
        let claimed = refetch(&store, &job.id);
        assert_eq!(claimed.attempts, 1);
        fail_or_retry(&store, &Job { attempts: 0, ..claimed.clone() }, &err).unwrap();
        let j = refetch(&store, &job.id);
        assert_eq!(j.status, JobStatus::Pending);

        // Fresh failure is inside the backoff window → not due.
        assert!(!job_due(&j, Utc::now()));
        // …but due once the window passes.
        assert!(job_due(&j, Utc::now() + chrono::Duration::seconds(21)));

        // Executions 2 and 3 → failed at the cap.
        store.mark_job_running(&job.id).unwrap();
        fail_or_retry(&store, &Job { attempts: 1, ..refetch(&store, &job.id) }, &err).unwrap();
        assert_eq!(refetch(&store, &job.id).status, JobStatus::Pending);

        store.mark_job_running(&job.id).unwrap();
        fail_or_retry(&store, &Job { attempts: 2, ..refetch(&store, &job.id) }, &err).unwrap();
        let j = refetch(&store, &job.id);
        assert_eq!(j.status, JobStatus::Failed);
        assert_eq!(j.attempts, 3);
    }

    fn refetch(store: &Store, id: &str) -> Job {
        store
            .get_pending_jobs(100)
            .unwrap()
            .into_iter()
            .find(|j| j.id == id)
            .unwrap_or_else(|| {
                // Not pending — read directly.
                store
                    .conn
                    .query_row(
                        "SELECT id, job_type, memory_id, status, attempts, last_error, created_at, updated_at
                         FROM jobs WHERE id = ?1",
                        [id],
                        |row| {
                            use std::str::FromStr;
                            Ok(Job {
                                id: row.get(0)?,
                                job_type: JobType::from_str(&row.get::<_, String>(1)?).unwrap(),
                                memory_id: row.get(2)?,
                                status: JobStatus::from_str(&row.get::<_, String>(3)?).unwrap(),
                                attempts: row.get(4)?,
                                last_error: row.get(5)?,
                                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?).unwrap().with_timezone(&Utc),
                                updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?).unwrap().with_timezone(&Utc),
                            })
                        },
                    )
                    .unwrap()
            })
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

        let (handle, task) = spawn_worker(db_path.clone(), worker_cfg());
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
