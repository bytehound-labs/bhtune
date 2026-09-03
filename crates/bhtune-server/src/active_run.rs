//! [`ActiveRun`]: tracks every tune run currently executing in a background task, so
//! [`crate::routes::runs`] can cancel one specific run and graceful shutdown can cancel and
//! wait for all in-flight runs to be restored before the process actually exits.
//!
//! Multiple tune tasks are allowed. A short-lived exclusive reservation is still used for
//! post-hoc PID writes/reverts, because those operations mutate a live loop directly and must
//! not overlap a tune or another write/revert.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bhtune_cli::cancel::CtrlCHandle;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

struct ActiveTask {
    cancel: CtrlCHandle,
    handle: JoinHandle<()>,
}

#[derive(Default)]
struct ActiveRunState {
    tasks: BTreeMap<i64, ActiveTask>,
    exclusive: Option<i64>,
}

/// Cheap to clone (an `Arc<Mutex<..>>` underneath), matching [`crate::state::AppState`]'s
/// own `Clone` requirement for axum's `State` extractor.
#[derive(Clone, Default)]
pub struct ActiveRun {
    inner: Arc<Mutex<ActiveRunState>>,
}

/// Returned by [`ActiveRun::start`] or [`ActiveRun::reserve`] when an exclusive write/revert
/// reservation is already held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunAlreadyActive {
    pub run_id: i64,
}

impl ActiveRun {
    /// Registers `task` as a background tune, recording `cancel` so a later
    /// [`ActiveRun::cancel`] or [`ActiveRun::cancel_and_wait`] call can trigger it.
    /// The registration is released automatically after the task returns or panics. If
    /// registration is rejected, `task` is dropped without being spawned, releasing anything
    /// it captured (such as Demo concurrency permits).
    ///
    /// Multiple tune tasks may be registered at the same time. The only conflict is with an
    /// exclusive post-hoc write/revert reservation, which protects direct live-loop mutation.
    pub async fn start(
        &self,
        run_id: i64,
        cancel: CtrlCHandle,
        task: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), RunAlreadyActive> {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard.exclusive {
            return Err(RunAlreadyActive { run_id: existing });
        }
        if guard.tasks.contains_key(&run_id) {
            return Err(RunAlreadyActive { run_id });
        }
        let task_handle = tokio::spawn(task);
        let active = self.clone();
        let handle = tokio::spawn(async move {
            if let Err(error) = task_handle.await {
                tracing::error!(run_id, %error, "tune task exited unexpectedly");
            }
            active.release(run_id).await;
        });
        guard.tasks.insert(run_id, ActiveTask { cancel, handle });
        Ok(())
    }

    /// Reserves the registry for a short operation the caller awaits directly -- currently
    /// only a post-hoc PID write or revert (`api-post-run-write`). Unlike
    /// [`ActiveRun::start`], nothing is spawned here: the caller must still call
    /// [`ActiveRun::release`] itself once its own operation finishes, on every exit path
    /// including an error return.
    ///
    /// The reservation is accepted only when all tune tasks and any other reservation have
    /// finished. This keeps direct live-loop mutation serialized without preventing independent
    /// tune tasks from running concurrently.
    pub async fn reserve(&self, run_id: i64) -> Result<(), RunAlreadyActive> {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard.exclusive {
            return Err(RunAlreadyActive { run_id: existing });
        }
        if let Some((&existing, _)) = guard.tasks.first_key_value() {
            return Err(RunAlreadyActive { run_id: existing });
        }
        guard.exclusive = Some(run_id);
        Ok(())
    }

    /// Returns the run id associated with the current exclusive write/revert reservation,
    /// if any. This is a cheap, non-authoritative pre-check; [`ActiveRun::start`] still
    /// re-checks under the same mutex to close the race with a reservation beginning between
    /// this call and the actual task registration.
    pub async fn exclusive_id(&self) -> Option<i64> {
        self.inner.lock().await.exclusive
    }

    /// Returns the ids of all registered background tune tasks. Cancellation always targets
    /// one id at a time.
    pub async fn active_run_ids(&self) -> Vec<i64> {
        self.inner.lock().await.tasks.keys().copied().collect()
    }

    /// Triggers cancellation for `run_id` if that tune is registered, exactly as if Ctrl+C
    /// had been pressed against an equivalent CLI-driven run. Returns whether a matching tune
    /// or exclusive reservation was found.
    pub async fn cancel(&self, run_id: i64) -> bool {
        let guard = self.inner.lock().await;
        if let Some(task) = guard.tasks.get(&run_id) {
            task.cancel.trigger();
            true
        } else {
            guard.exclusive == Some(run_id)
        }
    }

    /// Releases the tune registration or exclusive reservation for `run_id`. Spawned tune
    /// registrations release themselves; direct callers use this for exclusive reservations.
    /// A no-op if the id is not registered, which is expected when graceful shutdown has
    /// already taken the registry's entries for cancellation and waiting.
    pub async fn release(&self, run_id: i64) {
        let mut guard = self.inner.lock().await;
        if guard.exclusive == Some(run_id) {
            guard.exclusive = None;
        } else {
            guard.tasks.remove(&run_id);
        }
    }

    /// Triggers cancellation for every registered tune and waits up to `wait_timeout` for
    /// their background tasks to actually finish, including restore attempts. If the timeout
    /// elapses, the remaining tasks are abandoned and a loud error names all affected runs.
    pub async fn cancel_and_wait(&self, wait_timeout: Duration) {
        let (tasks, _exclusive) = {
            let mut guard = self.inner.lock().await;
            let tasks = std::mem::take(&mut guard.tasks);
            let exclusive = guard.exclusive.take();
            (tasks, exclusive)
        };
        if tasks.is_empty() {
            return;
        }
        let run_ids: Vec<i64> = tasks.keys().copied().collect();
        let tasks: Vec<_> = tasks
            .into_iter()
            .map(|(run_id, task)| {
                task.cancel.trigger();
                (run_id, task.cancel, task.handle)
            })
            .collect();
        let wait = async move {
            for (run_id, _cancel, handle) in tasks {
                if let Err(error) = handle.await {
                    tracing::error!(run_id, %error, "tune task exited unexpectedly");
                }
            }
        };
        if tokio::time::timeout(wait_timeout, wait).await.is_err() {
            tracing::error!(
                ?run_ids,
                ?wait_timeout,
                "tune tasks did not finish restoring within the shutdown grace period and were \
                 abandoned; affected loops may have been left mid-test -- check them by hand, \
                 or run `bhtune history revert` for the affected runs once this process has \
                 exited"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::sync::oneshot;

    async fn wait_until_inactive(active: &ActiveRun, run_id: i64) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while active.active_run_ids().await.contains(&run_id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the finished task should release its active-run registration");
    }

    #[tokio::test]
    async fn start_registers_runs_and_releases_a_completed_task() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = ran.clone();
        let (finish_tx, finish_rx) = oneshot::channel();
        active
            .start(1, handle, async move {
                ran_clone.store(true, Ordering::SeqCst);
                finish_rx.await.unwrap();
            })
            .await
            .unwrap();
        assert_eq!(active.active_run_ids().await, vec![1]);
        finish_tx.send(()).unwrap();
        wait_until_inactive(&active, 1).await;
        assert!(ran.load(Ordering::SeqCst));
        assert!(active.reserve(2).await.is_ok());
        active.release(2).await;
    }

    #[tokio::test]
    async fn start_allows_multiple_runs_while_they_are_active() {
        let active = ActiveRun::default();
        let (_ctrl_c_1, handle_1) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle_1, std::future::pending())
            .await
            .unwrap();

        let (_ctrl_c_2, handle_2) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(2, handle_2, std::future::pending())
            .await
            .unwrap();
        assert_eq!(active.active_run_ids().await, vec![1, 2]);
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
        assert!(active.active_run_ids().await.is_empty());
    }

    #[tokio::test]
    async fn releasing_one_run_keeps_other_runs_registered() {
        let active = ActiveRun::default();
        let (_ctrl_c_1, handle_1) = bhtune_cli::cancel::CtrlC::manual();
        let (_ctrl_c_2, handle_2) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle_1, std::future::pending())
            .await
            .unwrap();
        active
            .start(2, handle_2, std::future::pending())
            .await
            .unwrap();

        active.release(1).await;

        assert_eq!(active.active_run_ids().await, vec![2]);
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
        assert_eq!(active.active_run_ids().await, vec![1]);
    }

    #[tokio::test]
    async fn reserve_registers_an_exclusive_operation_without_spawning_anything() {
        let active = ActiveRun::default();
        active.reserve(1).await.unwrap();
        assert_eq!(active.exclusive_id().await, Some(1));
    }

    #[tokio::test]
    async fn reserve_refuses_while_a_spawned_task_is_active() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle, std::future::pending())
            .await
            .unwrap();
        let err = active.reserve(2).await.unwrap_err();
        assert_eq!(err, RunAlreadyActive { run_id: 1 });
    }

    #[tokio::test]
    async fn start_refuses_while_a_reservation_is_active() {
        let active = ActiveRun::default();
        active.reserve(1).await.unwrap();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        let err = active
            .start(2, handle, std::future::pending())
            .await
            .unwrap_err();
        assert_eq!(err, RunAlreadyActive { run_id: 1 });
    }

    #[tokio::test]
    async fn rejected_start_drops_the_unscheduled_task_and_keeps_the_reservation() {
        struct DropProbe(Arc<AtomicBool>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let active = ActiveRun::default();
        active.reserve(1).await.unwrap();
        let dropped = Arc::new(AtomicBool::new(false));
        let probe = DropProbe(dropped.clone());
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        let mut task = std::future::poll_fn(move |_| {
            let _ = &probe;
            Poll::Pending
        });
        let waker = std::task::Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(Pin::new(&mut task).poll(&mut context).is_pending());

        let err = active.start(2, handle, task).await.unwrap_err();

        assert_eq!(err, RunAlreadyActive { run_id: 1 });
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(active.exclusive_id().await, Some(1));
        assert!(active.active_run_ids().await.is_empty());
    }

    #[tokio::test]
    async fn cancel_and_wait_logs_an_unexpected_registration_task_failure() {
        let active = ActiveRun::default();
        let (_ctrl_c, cancel) = bhtune_cli::cancel::CtrlC::manual();
        let handle = tokio::spawn(async {
            panic!("simulated registration task failure");
        });
        active
            .inner
            .lock()
            .await
            .tasks
            .insert(1, ActiveTask { cancel, handle });

        active.cancel_and_wait(Duration::from_secs(1)).await;

        assert!(active.active_run_ids().await.is_empty());
    }

    #[tokio::test]
    async fn start_refuses_a_duplicate_run_id_while_the_task_is_active() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle, std::future::pending())
            .await
            .unwrap();

        let (_ctrl_c_duplicate, duplicate_handle) = bhtune_cli::cancel::CtrlC::manual();
        let err = active
            .start(1, duplicate_handle, std::future::pending())
            .await
            .unwrap_err();

        assert_eq!(err, RunAlreadyActive { run_id: 1 });
    }

    #[tokio::test]
    async fn reserve_refuses_a_second_reservation_while_one_is_active() {
        let active = ActiveRun::default();
        active.reserve(1).await.unwrap();
        let err = active.reserve(2).await.unwrap_err();
        assert_eq!(err, RunAlreadyActive { run_id: 1 });
    }

    #[tokio::test]
    async fn release_frees_a_reserved_slot() {
        let active = ActiveRun::default();
        active.reserve(1).await.unwrap();
        active.release(1).await;
        assert_eq!(active.exclusive_id().await, None);
    }

    #[tokio::test]
    async fn cancel_returns_true_for_a_reservation_but_has_nothing_to_trigger() {
        // A reservation has no `CtrlCHandle` at all, so this only proves `cancel` still
        // reports "yes, something is active for this id" -- matching a real tune task's
        // `cancel` return value -- rather than panicking or misreporting `false` just
        // because there's no cancellation machinery to fire.
        let active = ActiveRun::default();
        active.reserve(1).await.unwrap();
        assert!(active.cancel(1).await);
        // The reservation itself is untouched by a `cancel` call -- it isn't released.
        assert_eq!(active.exclusive_id().await, Some(1));
    }

    #[tokio::test]
    async fn cancel_and_wait_clears_a_reservation_immediately_without_waiting() {
        let active = ActiveRun::default();
        active.reserve(1).await.unwrap();
        // Must return promptly -- there is no `JoinHandle` to wait for, unlike a spawned
        // tune task.
        tokio::time::timeout(
            Duration::from_millis(200),
            active.cancel_and_wait(Duration::from_secs(5)),
        )
        .await
        .expect("cancel_and_wait must not block on a reservation with no task to await");
        assert_eq!(active.exclusive_id().await, None);
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
        // cancel_and_wait itself clears the registry (it takes the entries up front), not the
        // tasks -- confirms no entry leaks even though this test's task never calls `release()`.
        assert!(active.active_run_ids().await.is_empty());
    }

    #[tokio::test]
    async fn cancel_and_wait_waits_for_all_spawned_tasks() {
        let active = ActiveRun::default();
        let finished = Arc::new(AtomicUsize::new(0));

        for run_id in [1, 2] {
            let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
            let finished_clone = finished.clone();
            active
                .start(run_id, handle, async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    finished_clone.fetch_add(1, Ordering::SeqCst);
                })
                .await
                .unwrap();
        }

        tokio::time::timeout(
            Duration::from_millis(500),
            active.cancel_and_wait(Duration::from_secs(5)),
        )
        .await
        .expect("cancel_and_wait should resolve after all tasks finish");
        assert_eq!(finished.load(Ordering::SeqCst), 2);
        assert!(active.active_run_ids().await.is_empty());
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

    #[tokio::test]
    async fn cancel_and_wait_logs_and_consumes_a_panicking_task() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle, async {
                panic!("simulated task failure");
            })
            .await
            .unwrap();

        active.cancel_and_wait(Duration::from_secs(1)).await;
        assert!(active.active_run_ids().await.is_empty());
    }

    #[tokio::test]
    async fn a_panicking_task_releases_its_registration_without_shutdown() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle, async {
                panic!("simulated task failure");
            })
            .await
            .unwrap();

        wait_until_inactive(&active, 1).await;
        assert!(active.reserve(2).await.is_ok());
        active.release(2).await;
    }

    #[tokio::test]
    async fn a_task_that_reaches_its_own_timeout_releases_its_registration() {
        let active = ActiveRun::default();
        let (_ctrl_c, handle) = bhtune_cli::cancel::CtrlC::manual();
        active
            .start(1, handle, async {
                assert!(
                    tokio::time::timeout(Duration::from_millis(10), std::future::pending::<()>(),)
                        .await
                        .is_err()
                );
            })
            .await
            .unwrap();

        wait_until_inactive(&active, 1).await;
        assert!(active.reserve(2).await.is_ok());
        active.release(2).await;
    }
}
