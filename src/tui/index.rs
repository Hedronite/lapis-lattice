//! Search-index setup for the TUI: health-derived state, the explicit background
//! build and its progress. Files, editing and saves never wait on the index.

use std::time::{Duration, Instant};

use super::app::{App, Msg};

/// What a health answer says about the search index, beyond reachability.
pub(crate) struct IndexHealth {
    pub(crate) embedded: bool,
    pub(crate) built: bool,
    pub(crate) documents: u64,
}

/// Search-index lifecycle as the TUI presents it. Files never wait on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexState {
    /// No health answer yet.
    Unknown,
    /// The embedded index was never built: search has nothing, notes still open and save.
    Missing,
    Indexing {
        done: u64,
        total: u64,
    },
    Ready(u64),
    Failed(String),
    /// An HTTP lattice owns its own index lifecycle.
    Managed,
}

impl IndexState {
    /// Missing or failed: the user has a build action to take.
    pub(crate) fn needs_setup(&self) -> bool {
        matches!(self, IndexState::Missing | IndexState::Failed(_))
    }

    /// The state after a health answer. A build in flight owns the state, and a
    /// failure stays visible until retried instead of reverting to plain "missing".
    pub(crate) fn observe(&self, health: &IndexHealth) -> IndexState {
        let reported = if !health.embedded {
            IndexState::Managed
        } else if health.built {
            IndexState::Ready(health.documents)
        } else {
            IndexState::Missing
        };
        match (self, reported) {
            (IndexState::Indexing { .. }, _) | (IndexState::Failed(_), IndexState::Missing) => self.clone(),
            (_, reported) => reported,
        }
    }
}

impl App {
    /// Build the embedded index on a blocking worker and post progress. The tree,
    /// editor and saves stay available; paths saved meanwhile are indexed after it.
    pub(crate) fn build_index(&mut self) {
        const MANAGED: &str = "search index is managed by the configured HTTP lattice";
        match self.index {
            IndexState::Indexing { .. } => {
                self.set_status("index build already running · files remain usable");
                return;
            }
            IndexState::Managed => {
                self.set_status(MANAGED);
                return;
            }
            _ => {}
        }
        let backend = match self.ctx.backend() {
            Ok(backend) if backend.mode() == "embedded" => backend,
            Ok(_) => {
                self.index = IndexState::Managed;
                self.set_status(MANAGED);
                return;
            }
            Err(e) => {
                self.set_status(format!("index build failed: {e} · Space i retries"));
                self.index = IndexState::Failed(e.to_string());
                return;
            }
        };
        self.index = IndexState::Indexing { done: 0, total: 0 };
        self.set_status("indexing in the background · files remain usable");
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let mut sent = Instant::now();
            let report = backend.reindex_all_with_progress(&mut |done, total| {
                if done == 0 || done == total || sent.elapsed() >= Duration::from_millis(100) {
                    sent = Instant::now();
                    let _ = tx.send(Msg::IndexProgress(done, total));
                }
            });
            let _ = tx.send(Msg::IndexBuilt(report.map(|r| r.documents).map_err(|e| e.to_string())));
        });
    }

    pub(crate) fn index_progress(&mut self, done: u64, total: u64) {
        if matches!(self.index, IndexState::Indexing { .. }) {
            self.index = IndexState::Indexing { done, total };
            // Keep the count in the status line while it shows the build, so progress
            // stays visible even when the right-hand hint has no room. Any other
            // message (a guard, a save) keeps its place.
            if self.status.starts_with("indexing ") {
                self.set_status(format!("indexing {done}/{total} · files remain usable"));
            }
        }
    }

    pub(crate) fn index_built(&mut self, result: std::result::Result<u64, String>) {
        match result {
            Ok(documents) => {
                self.index = IndexState::Ready(documents);
                self.set_status(format!("indexed {documents} notes · search is ready"));
                for rel in self.index_pending.take() {
                    self.kick(rel);
                }
            }
            Err(e) => {
                self.index_pending.borrow_mut().clear();
                self.set_status(format!("index build failed: {e} · Space i retries · files unaffected"));
                self.index = IndexState::Failed(e);
            }
        }
        self.poll_health();
    }
}

#[cfg(test)]
mod tests {
    use super::{IndexHealth, IndexState};

    fn health(embedded: bool, built: bool, documents: u64) -> IndexHealth {
        IndexHealth { embedded, built, documents }
    }

    #[test]
    fn health_answers_map_to_setup_states() {
        let unknown = IndexState::Unknown;
        assert_eq!(unknown.observe(&health(true, false, 0)), IndexState::Missing);
        assert!(IndexState::Missing.needs_setup());
        assert_eq!(
            unknown.observe(&health(true, true, 0)),
            IndexState::Ready(0),
            "built with no notes is ready"
        );
        assert_eq!(unknown.observe(&health(true, true, 12)), IndexState::Ready(12));
        assert_eq!(unknown.observe(&health(false, false, 0)), IndexState::Managed);
        assert!(!IndexState::Managed.needs_setup());
    }

    #[test]
    fn a_build_in_flight_and_a_failure_survive_health_polls() {
        let building = IndexState::Indexing { done: 3, total: 9 };
        assert_eq!(building.observe(&health(true, false, 0)), building);
        let failed = IndexState::Failed("disk full".into());
        assert_eq!(failed.observe(&health(true, false, 0)), failed);
        assert!(failed.needs_setup());
        assert_eq!(failed.observe(&health(true, true, 5)), IndexState::Ready(5));
    }
}
