//! In-process registry of which background phases are running *right now*,
//! for the viewer's live-activity panel. Phases are coarse `&'static str`
//! labels ("enrich", "consolidate", "graph_build"…) tracked by an active
//! count, so concurrent or nested `begin`s on the same phase are safe.
//!
//! ponytail: in-memory, single-process, lost on restart — which is the whole
//! point (it reflects *this* engine's current work, not history). The `jobs`
//! table already persists durable backlog; wire a cross-process feed only if
//! that ever becomes a need.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Activity {
    phases: Mutex<BTreeMap<&'static str, usize>>,
}

impl Activity {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Mark `phase` active until the returned guard drops. Poison-tolerant:
    /// a panicked holder must not wedge the whole status panel.
    pub fn begin(self: &Arc<Self>, phase: &'static str) -> ActivityGuard {
        if let Ok(mut p) = self.phases.lock() {
            *p.entry(phase).or_insert(0) += 1;
        }
        ActivityGuard { activity: Arc::clone(self), phase }
    }

    /// Phases with at least one active holder, sorted (BTreeMap order).
    pub fn snapshot(&self) -> Vec<&'static str> {
        self.phases
            .lock()
            .map(|p| p.iter().filter(|&(_, &n)| n > 0).map(|(&k, _)| k).collect())
            .unwrap_or_default()
    }
}

pub struct ActivityGuard {
    activity: Arc<Activity>,
    phase: &'static str,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        if let Ok(mut p) = self.activity.phases.lock()
            && let Some(n) = p.get_mut(self.phase)
        {
            *n = n.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_raises_then_clears_phase_with_nesting() {
        let a = Activity::new();
        assert!(a.snapshot().is_empty());
        {
            let _g = a.begin("consolidate");
            assert_eq!(a.snapshot(), vec!["consolidate"]);
            {
                let _g2 = a.begin("consolidate");
                let _g3 = a.begin("enrich");
                // nested same-phase + a second phase, both active
                assert_eq!(a.snapshot(), vec!["consolidate", "enrich"]);
            }
            // inner guards dropped; outer "consolidate" still held
            assert_eq!(a.snapshot(), vec!["consolidate"]);
        }
        assert!(a.snapshot().is_empty(), "all guards dropped ⇒ nothing active");
    }
}
