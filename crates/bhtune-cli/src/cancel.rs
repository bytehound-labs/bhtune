//! A single, process-wide Ctrl+C listener shared by every await point in a tune, replacing
//! the pre-`safety-cancellation` design of constructing `tokio::signal::ctrl_c()` fresh on
//! every polling-loop iteration (see AGENTS.md's `safety-cancellation`). Registering the
//! signal exactly once, as early in the process as possible, closes the gap where a Ctrl+C
//! delivered while no listener happens to be alive is silently swallowed -- tokio installs a
//! process-wide `SIGINT` handler the first time `ctrl_c()` is polled and never reverts to the
//! OS default, so a lost signal isn't merely unhandled, it's gone.
//!
//! Built on [`tokio::sync::watch`] rather than `tokio_util::sync::CancellationToken` (which
//! would need a new dependency) specifically for its per-clone "have I observed this value
//! yet" semantics: a fresh [`CtrlC::signalled`] call after a signal already fired resolves
//! immediately (including one that arrived before this handle's first `signalled()` call at
//! all), and a *second* signal is a second, distinguishable resolution on the same handle --
//! exactly the two states `safety-cancellation` needs to tell apart (first Ctrl+C aborting
//! the run, versus a second one during the restore forcing it to give up).

use std::future::Future;

use tokio::sync::watch;

/// A handle to the process's Ctrl+C signal, threaded explicitly through every function that
/// needs to react to it (`execute`, `run_polling_loop`, `attempt_restore`) rather than each
/// calling `tokio::signal::ctrl_c()` itself.
///
/// [`CtrlC::install`] must be called exactly once, as early as possible, in the real binary's
/// startup ([`crate::run`]) -- never from a function unit tests exercise. `run_with_cli`'s
/// and `commands::tune::run`'s test-facing entry points instead default to [`CtrlC::never`]
/// internally, so the many unit tests that exercise those functions never install a real
/// process-wide signal handler. That matters beyond just those tests: once *anything* in a
/// process calls `tokio::signal::ctrl_c()`, the OS's default "terminate on SIGINT" behavior
/// is gone for the rest of that process, so if any unit test installed a real handler, a
/// developer's own Ctrl+C meant to abort a hung `cargo test` run could silently disappear
/// into an idle listener nothing is polling.
///
/// `pub` (rather than `pub(crate)`) so `bhtune-server` can name the type as it threads a
/// [`CtrlC::manual`] handle through `commands::tune::drive` for an HTTP-triggered run -- see
/// that constructor's doc comment. [`CtrlC::signalled`] itself deliberately stays
/// `pub(crate)`: only code inside this crate (`execute`/`run_polling_loop`/`attempt_restore`)
/// ever needs to *observe* a cancellation, an external caller only ever needs to *trigger*
/// one, via a [`CtrlCHandle`].
pub struct CtrlC {
    rx: watch::Receiver<u32>,
}

fn install_with_signal_provider<F, Fut>(signal_provider: F) -> CtrlC
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), std::io::Error>> + Send + 'static,
{
    let (tx, rx) = watch::channel(0u32);
    tokio::spawn(async move {
        let mut count = 0u32;
        loop {
            if signal_provider().await.is_err() {
                // The OS-level listener itself failed to install/poll (e.g. an exhausted
                // signal-handling resource -- vanishingly rare). Stop rather than spin; a
                // `CtrlC` handle simply never fires again for the rest of this process,
                // the same observable behavior as never receiving a signal at all.
                return;
            }
            count = count.wrapping_add(1);
            if tx.send(count).is_err() {
                // Every receiver was dropped -- nothing left to notify.
                return;
            }
        }
    });
    CtrlC { rx }
}

impl CtrlC {
    /// Spawns the one long-lived task that listens for Ctrl+C for the rest of the process's
    /// life, incrementing a counter on every delivery -- see the struct doc comment for why
    /// this must be called exactly once, and only from real process startup.
    pub(crate) fn install() -> CtrlC {
        // Fire-and-forget: nothing ever awaits or aborts this task, so its `JoinHandle` is
        // simply never bound (an explicit `let _ = ...` would trip clippy's
        // `let_underscore_future`, which can't tell this apart from a future that was meant
        // to run but never got polled -- this one is already spawned onto the runtime the
        // moment `tokio::spawn` returns).
        install_with_signal_provider(tokio::signal::ctrl_c)
    }

    /// Resolves the next time Ctrl+C is delivered -- immediately, if one already arrived
    /// since this handle last observed a change, including one that arrived before this
    /// method was ever called (e.g. during a slow startup sequence -- see
    /// `safety-cancellation`'s emergent pre-polling-loop behavior in AGENTS.md).
    pub(crate) async fn signalled(&mut self) {
        // A real `install()`-backed handle's sender loops for the process's entire life, and
        // a `never()` handle deliberately leaks its sender (see below) -- so `changed()`'s
        // `Err` (every sender dropped) case is not expected to occur in practice. Treated as
        // "never resolves" rather than unwrapped/panicking, since a hung await is a far
        // safer failure mode here than a panic in the middle of a live tuning run.
        while self.rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// A handle that never fires -- for call paths that don't exercise cancellation and must
    /// never install a real process-wide signal handler (see the struct doc comment).
    #[cfg(test)]
    pub(crate) fn never() -> CtrlC {
        let (tx, rx) = watch::channel(0u32);
        // Leaked deliberately: dropping `tx` here would make `rx.changed()` resolve
        // immediately with an `Err`, the opposite of "never fires". `mem::forget` (not
        // `Box::leak`) since there's no heap allocation to leak, just the drop glue to skip.
        std::mem::forget(tx);
        CtrlC { rx }
    }

    /// Returns a fresh `(CtrlC, Sender)` pair so a test can manually `.send(..)` to simulate a
    /// Ctrl+C press deterministically, without a real OS signal or subprocess.
    #[cfg(test)]
    pub(crate) fn test_pair() -> (CtrlC, watch::Sender<u32>) {
        let (tx, rx) = watch::channel(0u32);
        (CtrlC { rx }, tx)
    }

    /// Returns a fresh `(CtrlC, CtrlCHandle)` pair for a caller with no real OS Ctrl+C
    /// keypress to listen for at all -- `bhtune-server`'s background tune task, which needs
    /// an HTTP request (`POST /api/runs/{id}/cancel`, or graceful shutdown) to be able to
    /// trigger the exact same cancellation an interactive Ctrl+C would.
    ///
    /// Deliberately **not** `#[cfg(test)]`-gated, unlike [`CtrlC::never`]/[`CtrlC::test_pair`]
    /// above: those exist purely so this crate's own unit tests can avoid installing a real
    /// process-wide signal handler, but `manual()` never touches
    /// `tokio::signal`/[`CtrlC::install`] at all, so calling it from production code any
    /// number of times (once per in-flight run) carries none of that risk.
    pub fn manual() -> (CtrlC, CtrlCHandle) {
        let (tx, rx) = watch::channel(0u32);
        (CtrlC { rx }, CtrlCHandle { tx })
    }
}

/// A trigger for a [`CtrlC`] handle created via [`CtrlC::manual`] -- the HTTP-triggered
/// equivalent of a real Ctrl+C keypress. Deliberately a thin wrapper around the same
/// `watch::Sender<u32>` mechanism `#[cfg(test)]`'s `test_pair()` already uses internally,
/// rather than a second, parallel cancellation mechanism: [`CtrlC::signalled`] can't tell the
/// two apart, so `execute`/`run_polling_loop`/`attempt_restore` need no changes at all to
/// support HTTP-triggered cancellation.
///
/// `Clone` so a caller can store one copy in a run registry (to answer a later
/// `POST /api/runs/{id}/cancel`) while another copy is held by whatever's waiting to trigger
/// it on graceful shutdown.
#[derive(Clone)]
pub struct CtrlCHandle {
    tx: watch::Sender<u32>,
}

impl CtrlCHandle {
    /// Requests cancellation, exactly as if Ctrl+C had been pressed. Safe to call more than
    /// once -- a second call is exactly what lets a caller model a "second Ctrl+C" hard-exit
    /// request arriving during an already-in-flight restore, matching `safety-cancellation`'s
    /// interactive CLI behavior (see AGENTS.md) -- and safe to call after the paired
    /// [`CtrlC`] has already been dropped (the run this handle was for has already ended):
    /// [`watch::Sender::send_modify`] never fails, unlike `send`, so there is nothing to
    /// propagate or ignore.
    pub fn trigger(&self) {
        self.tx.send_modify(|count| *count = count.wrapping_add(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn never_does_not_resolve_signalled_even_after_a_yield() {
        let mut ctrl_c = CtrlC::never();
        tokio::select! {
            () = ctrl_c.signalled() => panic!("never() must not resolve"),
            () = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }

    #[tokio::test]
    async fn test_pair_resolves_signalled_after_a_manual_send() {
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();
        tokio::time::timeout(Duration::from_millis(200), ctrl_c.signalled())
            .await
            .expect("signalled() should resolve promptly after a manual send");
    }

    #[tokio::test]
    async fn signalled_resolves_again_for_a_second_send_on_the_same_handle() {
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();
        ctrl_c.signalled().await;
        tx.send(2).unwrap();
        tokio::time::timeout(Duration::from_millis(200), ctrl_c.signalled())
            .await
            .expect("a second send should resolve signalled() again");
    }

    #[tokio::test]
    async fn a_send_before_the_receiver_ever_awaits_is_still_observed() {
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();
        // No intervening await -- `watch`'s "already changed, not yet observed by this
        // receiver" semantics must still resolve `signalled()` immediately, matching a real
        // Ctrl+C delivered before a caller ever starts waiting for it.
        tokio::time::timeout(Duration::from_millis(50), ctrl_c.signalled())
            .await
            .expect("a send delivered before signalled() was ever called must still be seen");
    }

    #[tokio::test]
    async fn signal_listener_stops_when_signal_provider_fails() {
        let called = std::sync::Arc::new(tokio::sync::Notify::new());
        let provider_called = std::sync::Arc::clone(&called);
        let _ctrl_c = install_with_signal_provider(move || {
            provider_called.notify_one();
            async { Err(std::io::Error::other("test signal provider failure")) }
        });

        tokio::time::timeout(Duration::from_millis(200), called.notified())
            .await
            .expect("the injected signal provider should be called");
    }

    #[tokio::test]
    async fn signal_listener_stops_when_all_receivers_are_dropped() {
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let provider_started = std::sync::Arc::clone(&started);
        let provider_release = std::sync::Arc::clone(&release);
        let ctrl_c = install_with_signal_provider(move || {
            provider_started.notify_one();
            let provider_release = std::sync::Arc::clone(&provider_release);
            async move {
                provider_release.notified().await;
                Ok(())
            }
        });

        tokio::time::timeout(Duration::from_millis(200), started.notified())
            .await
            .expect("the injected signal provider should start");
        drop(ctrl_c);
        release.notify_one();
        tokio::task::yield_now().await;
    }

    // `install()`'s own real-signal-handling behavior deliberately has no in-process test
    // here: raising a real `SIGINT` against this test binary before `install()`'s spawned
    // task has actually reached `tokio::signal::ctrl_c().await` (registering the OS-level
    // handler) would hit the OS default disposition instead -- process termination -- and
    // there is no race-free way to know that registration has happened from outside the
    // task. `tests/ctrlc_abort.rs` already proves `install()` against a real `SIGINT` safely,
    // by sending it to a dedicated child *process* rather than this shared test binary.

    #[tokio::test]
    async fn manual_resolves_signalled_after_a_trigger() {
        let (mut ctrl_c, handle) = CtrlC::manual();
        handle.trigger();
        tokio::time::timeout(Duration::from_millis(200), ctrl_c.signalled())
            .await
            .expect("signalled() should resolve promptly after trigger()");
    }

    #[tokio::test]
    async fn manual_does_not_resolve_signalled_before_any_trigger() {
        let (mut ctrl_c, _handle) = CtrlC::manual();
        tokio::select! {
            () = ctrl_c.signalled() => panic!("signalled() must not resolve before trigger()"),
            () = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }

    #[tokio::test]
    async fn manual_handle_clone_triggers_the_same_ctrl_c() {
        let (mut ctrl_c, handle) = CtrlC::manual();
        let cloned = handle.clone();
        cloned.trigger();
        tokio::time::timeout(Duration::from_millis(200), ctrl_c.signalled())
            .await
            .expect("a clone's trigger() should resolve the original CtrlC's signalled()");
    }

    #[tokio::test]
    async fn manual_handle_trigger_is_safe_to_call_after_ctrl_c_is_dropped() {
        let (ctrl_c, handle) = CtrlC::manual();
        drop(ctrl_c);
        // Must not panic even though every receiver is gone.
        handle.trigger();
    }

    #[tokio::test]
    async fn manual_handle_second_trigger_resolves_signalled_again() {
        let (mut ctrl_c, handle) = CtrlC::manual();
        handle.trigger();
        tokio::time::timeout(Duration::from_millis(200), ctrl_c.signalled())
            .await
            .expect("the first trigger should resolve signalled()");
        handle.trigger();
        tokio::time::timeout(Duration::from_millis(200), ctrl_c.signalled())
            .await
            .expect("a second trigger() should resolve signalled() again");
    }
}
