//! [`ActiveRun`]: tracks the single tune run (if any) currently executing in a background
//! task, so [`crate::routes::runs`] can refuse a second concurrent run, find the right
//! handle to cancel, and let graceful shutdown cancel and wait for an in-flight run to be
//! restored before the process actually exits.
//!
//! v1 deliberately allows only **one** active run at a time, matching the CLI's own
//! single-sequential-process model: a second `POST /api/runs` while one is already running
//! gets `409 Conflict` rather than starting a concurrent `OpcDaDriver` against the same live
//! plant, which is untested territory this project isn't taking on yet. See AGENTS.md's
//! `server-start-tune-api` notes.

use std::sync::Arc;
use std::time::Duration;

use bhtune_cli::cancel::CtrlCHandle;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

struct ActiveRunEntry {
    run_id: i64,
    cancel: CtrlCHandle,
    handle: JoinHandle<()>,
}

/// Cheap to clone (an `Arc<Mutex<..>>` underneath), matching [`crate::state::AppState`]'s own
/// `Clone` requirement for axum's `State` extractor.
#[derive(Clone, Default)]
pub struct ActiveRun {
    inner: Arc<Mutex<Option<ActiveRunEntry>>>,
}

/// Returned by [`ActiveRun::start`] when the single-run slot is already taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunAlreadyActive {
    pub run_id: i64,
}

impl ActiveRun {
    /// Reserves the single active-run slot for `run_id` and spawns `task` as its background
    /// execution, recording `cancel` so a later [`ActiveRun::cancel`]/
    /// [`ActiveRun::cancel_and_wait`] call can trigger it. `task` must itself call
    /// [`ActiveRun::release`] when it finishes (however it finishes) -- this method does not
    /// do that on the caller's behalf, since only the task itself knows when its work is
    /// actually done.
    ///
    /// Returns the currently active run's id (wrapped in [`RunAlreadyActive`]) instead of
    /// starting `task` at all if a run is already active.
    pub async fn start(
        &self,
        run_id: i64,
        cancel: CtrlCHandle,
        task: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), RunAlreadyActive> {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard.as_ref() {
            return Err(RunAlreadyActive {
                run_id: existing.run_id,
            });
        }
        let handle = tokio::spawn(task);
        *guard = Some(ActiveRunEntry {
            run_id,
            cancel,
            handle,
        });
        Ok(())
    }

    /// The currently active run's id, if any -- a cheap, non-authoritative check `POST
    /// /api/runs`'s handler uses *before* calling `prepare()` (template lookup, tag
    /// derivation, a real driver connection attempt) purely to avoid that work when it's
    /// already obvious a run is active. Not a substitute for [`ActiveRun::start`]'s own
    /// atomic reservation: a run can start or finish between this call returning and
    /// whatever the caller does next, which is exactly why `start` re-checks and is the one
    /// that actually decides whether a new run may proceed.
    pub async fn active_run_id(&self) -> Option<i64> {
        self.inner.lock().await.as_ref().map(|e| e.run_id)
    }

    /// Triggers cancellation for `run_id` if it is the currently active run, exactly as if
    /// Ctrl+C had been pressed against an equivalent CLI-driven run. Returns whether a
    /// matching run was found (and thus cancelled) -- `false` covers both "nothing is
    /// active" and "a *different* run is active", which a caller (the `POST
    /// /api/runs/{id}/cancel` handler) deliberately doesn't need to distinguish: either way,
    /// `id` is not a run this call can cancel right now.
    pub async fn cancel(&self, run_id: i64) -> bool {
        let guard = self.inner.lock().await;
        match guard.as_ref() {
            Some(entry) if entry.run_id == run_id => {
                entry.cancel.trigger();
                true
            }
            _ => false,
        }
    }

    /// Releases the slot -- called by a run's own spawned task once it finishes, however it
    /// finished, so the slot is free for the next `POST /api/runs`. A no-op if `run_id` isn't
    /// (or is no longer) the active run, which is always the case if
    /// [`ActiveRun::cancel_and_wait`] already claimed the slot for shutdown.
    pub async fn release(&self, run_id: i64) {
        let mut guard = self.inner.lock().await;
        if matches!(guard.as_ref(), Some(entry) if entry.run_id == run_id) {
            *guard = None;
        }
    }

    /// If a run is active, triggers its cancellation and waits up to `wait_timeout` for its
    /// background task to actually finish (which only happens once `drive()`'s restore
    /// attempt has resolved, one way or another) -- used by graceful shutdown so the process
    /// doesn't exit while a loop still sits at a relay-test MV/mode with a cancellation
    /// request nobody waited for.
    ///
    /// If `wait_timeout` elapses first, the task is abandoned (it keeps running detached
    /// until the process actually exits, at which point it is simply dropped) and a loud
    /// `tracing::error!` names the run -- the same risk, and the same documented recovery
    /// path (`bhtune history revert`/a future `bhtune restore-loop`), as a hard `kill -9` of
    /// a CLI-driven tune, which this project already accepts rather than claims to prevent.
    pub async fn cancel_and_wait(&self, wait_timeout: Duration) {
        let entry = self.inner.lock().await.take();
        let Some(entry) = entry else { return };
        tracing::warn!(
            run_id = entry.run_id,
            ?wait_timeout,
            "shutdown requested while a tune is in flight; cancelling and waiting for the \
             loop to be restored before exiting"
        );
        entry.cancel.trigger();
        if tokio::time::timeout(wait_timeout, entry.handle)
            .await
            .is_err()
        {
            tracing::error!(
                run_id = entry.run_id,
                "tune did not finish restoring within the shutdown grace period and was \
                 abandoned; the loop may have been left mid-test -- check it by hand, or run \
                 `bhtune history revert` for this run once this process has exited"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn start_reserves_the_slot_and_runs_the_task() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = ran.clone();
        active
            .start(1, handle, async move {
                ran_clone.store(true, Ordering::SeqCst);
            })
            .await
            .unwrap();
        assert_eq!(active.active_run_id().await, Some(1));
        // Give the spawned task a chance to actually run.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn start_refuses_a_second_run_while_one_is_active() {
        let active = ActiveRun::default();
        let (_ctrl_c_1, handle_1) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle_1, std::future::pending())
            .await
            .unwrap();

        let (_ctrl_c_2, handle_2) = bhtune_cli::cancel::CtrlC::manual();
        let err = active
            .start(2, handle_2, std::future::pending())
            .await
            .unwrap_err();
        assert_eq!(err, RunAlreadyActive { run_id: 1 });
    }

    #[tokio::test]
    async fn release_frees_the_slot_for_the_matching_run_id() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle, std::future::pending())
            .await
            .unwrap();
        active.release(1).await;
        assert_eq!(active.active_run_id().await, None);
    }

    #[tokio::test]
    async fn release_is_a_no_op_for_a_non_matching_run_id() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle, std::future::pending())
            .await
            .unwrap();
        active.release(999).await;
        assert_eq!(active.active_run_id().await, Some(1));
    }

    #[tokio::test]
    async fn cancel_returns_true_for_the_matching_run_id() {
        // `CtrlC::signalled` (the observing half) is deliberately `pub(crate)` to
        // `bhtune-cli` -- see its doc comment -- so this module's own tests can't directly
        // observe that `trigger()` fired the paired `CtrlC`. That propagation is already
        // covered by `bhtune-cli::cancel`'s own test suite; what belongs here is only
        // `ActiveRun::cancel`'s own dispatch logic (right id -> `true`), and the full
        // end-to-end proof that cancellation actually reaches and stops a running tune
        // lives in `routes::runs`'s route-level tests.
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle, std::future::pending())
            .await
            .unwrap();
        assert!(active.cancel(1).await);
    }

    #[tokio::test]
    async fn cancel_returns_false_for_a_non_matching_run_id() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle, std::future::pending())
            .await
            .unwrap();
        assert!(!active.cancel(999).await);
    }

    #[tokio::test]
    async fn cancel_returns_false_when_nothing_is_active() {
        let active = ActiveRun::default();
        assert!(!active.cancel(1).await);
    }

    #[tokio::test]
    async fn cancel_and_wait_is_a_no_op_when_nothing_is_active() {
        let active = ActiveRun::default();
        // Must return promptly -- there is nothing to wait for.
        tokio::time::timeout(
            Duration::from_millis(200),
            active.cancel_and_wait(Duration::from_secs(5)),
        )
        .await
        .expect("cancel_and_wait must not block when no run is active");
    }

    #[tokio::test]
    async fn cancel_and_wait_waits_for_the_spawned_task_to_actually_finish() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_clone = finished.clone();
        // A task that takes a little while to actually stop (simulating an in-flight
        // restore completing) rather than resolving the instant it's spawned -- proves
        // `cancel_and_wait` really awaits the `JoinHandle` to completion, rather than
        // returning as soon as `trigger()` is called. (Whether `trigger()` itself reaches a
        // real `CtrlC`'s `signalled()` is already covered by `bhtune-cli::cancel`'s own
        // tests; `CtrlC::signalled` is deliberately `pub(crate)` there and unobservable from
        // this crate -- see its doc comment.)
        active
            .start(1, handle, async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                finished_clone.store(true, Ordering::SeqCst);
            })
            .await
            .unwrap();

        tokio::time::timeout(
            Duration::from_millis(500),
            active.cancel_and_wait(Duration::from_secs(5)),
        )
        .await
        .expect("cancel_and_wait should resolve once the task finishes");
        assert!(
            finished.load(Ordering::SeqCst),
            "cancel_and_wait must not return before the task's future has fully resolved"
        );
        // cancel_and_wait itself clears the slot (it `take()`s the entry up front), not the
        // task -- confirms the slot doesn't leak even though this test's task never calls
        // `release()` itself.
        assert_eq!(active.active_run_id().await, None);
    }

    #[tokio::test]
    async fn cancel_and_wait_abandons_a_task_that_does_not_finish_within_the_timeout() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        // A task that ignores cancellation entirely (simulating a stuck restore) -- proves
        // `cancel_and_wait` still returns rather than blocking forever.
        active
            .start(1, handle, std::future::pending())
            .await
            .unwrap();

        tokio::time::timeout(
            Duration::from_millis(500),
            active.cancel_and_wait(Duration::from_millis(50)),
        )
        .await
        .expect("cancel_and_wait must respect its own timeout even if the task never exits");
    }
}
