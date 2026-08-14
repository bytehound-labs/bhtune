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
pub(crate) struct CtrlC {
    rx: watch::Receiver<u32>,
}

impl CtrlC {
    /// Spawns the one long-lived task that listens for Ctrl+C for the rest of the process's
    /// life, incrementing a counter on every delivery -- see the struct doc comment for why
    /// this must be called exactly once, and only from real process startup.
    pub(crate) fn install() -> CtrlC {
        let (tx, rx) = watch::channel(0u32);
        // Fire-and-forget: nothing ever awaits or aborts this task, so its `JoinHandle` is
        // simply never bound (an explicit `let _ = ...` would trip clippy's
        // `let_underscore_future`, which can't tell this apart from a future that was meant
        // to run but never got polled -- this one is already spawned onto the runtime the
        // moment `tokio::spawn` returns).
        tokio::spawn(async move {
            let mut count = 0u32;
            loop {
                if tokio::signal::ctrl_c().await.is_err() {
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

    // `install()`'s own real-signal-handling behavior deliberately has no in-process test
    // here: raising a real `SIGINT` against this test binary before `install()`'s spawned
    // task has actually reached `tokio::signal::ctrl_c().await` (registering the OS-level
    // handler) would hit the OS default disposition instead -- process termination -- and
    // there is no race-free way to know that registration has happened from outside the
    // task. `tests/ctrlc_abort.rs` already proves `install()` against a real `SIGINT` safely,
    // by sending it to a dedicated child *process* rather than this shared test binary.
}
