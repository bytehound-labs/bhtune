# Live-plant safety

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Safety (`cli-safety`)

Guardrails for unattended operation against live plant equipment — the legacy app is always
human-attended (an operator watches the trend and can hit Stop); scheduled/scripted tuning
removes that supervision while still stroking a live valve, so these are not optional polish:

- **`LoopConfig::validate`** (`bhtune-core`) — real range validation on `relay_amp_percent` at
  the model/construction level, not just a client-side keystroke filter or a single "not
  blank" check (the legacy predecessor's bug: a leftover 2014/2015/2016 debug code left in the
  Relay Amplitude box passed its only check). `RELAY_AMP_PERCENT_MIN = 0.1`/
  `RELAY_AMP_PERCENT_MAX = 50.0` (both `pub const f32` on `LoopConfig`) reject non-finite
  values and anything outside that range; `LoopConfigError` is a hand-rolled `Display`/
  `std::error::Error` enum (no `thiserror` — `bhtune-core` stays dependency-minimal) — the
  crate's first fallible-construction pattern (previously only `Option`-returning functions
  existed, e.g. `derive_tag`). `build_loop_config` (`commands/tune.rs`) calls `.validate()?`
  immediately after constructing the `LoopConfig`, before any driver connection or database
  write, mirroring the `--write-pid`-requires-`--yes` fail-fast precedent below.
- **`[tuning].timeout_secs`** (built-in default `3600`) — a mandatory wall-clock limit on the
  whole test, with no disable/unlimited option. The global value is validated before any driver
  connection or database/live-loop mutation. Implemented in `run_polling_loop` as a
  `tokio::time::sleep` created once before the loop and raced via a `tokio::select!` arm
  alongside `interval.tick()` and a single process-wide `CtrlC` handle (see
  `safety-cancellation` below) — but that outer race only covers the _idle_ wait between ticks.
  The timeout (and Ctrl+C) also stay effective _during_ a tick — including a stalled driver
  read or write, e.g. a wedged DCOM call or a black-holed network — because every driver call
  inside the tick body is separately raced against the same `CtrlC` handle and the global
  `[tuning].op_timeout_secs` cap via `bounded_driver_call` (see `safety-cancellation` for why
  this two-layer design, rather than one that only checked after each completed tick, was
  needed).
  On firing, the loop is restored to its pre-test mode via the exact same path as a Ctrl+C
  abort (`restore` + `TuneRunRow::abort`, recording plain `Aborted` in `tune_runs.outcome` —
  no new DB state) and reported to the caller as the distinct `TuneOutcome::TimedOut` /
  `EXIT_TIMED_OUT = 4`, so a scheduler's alerting can tell "this run had to be forcibly killed
  for running too long" (possibly a stuck relay, a misconfigured tag mapping, or a stalled
  driver read — worth investigating) apart from "an operator stopped it on purpose"
  (`EXIT_ABORTED`, routine).
- **`--write-pid <level>` unconditionally requires `--yes`** — in `run()`
  (`args.write_pid.is_some() && !args.yes`), checked before any driver connection or
  database write. There is no rehearsal mode that lifts this gate: a `--write-pid` run either
  has explicit confirmation or is rejected outright. (An earlier `--dry-run` flag did lift
  this gate, but it was removed since it did not actually avoid touching the live loop: it
  still forced the mode transition and stroked the MV through a full relay test, only
  skipping the final PID write.)

**Testing approach.** `run_times_out_and_aborts_when_timeout_secs_elapses_before_completion`
exercises the real timeout end-to-end (`poll_interval_ms: 3`/`timeout_secs: 1`, deliberately
not a multiple of each other so the deadline never lands exactly on a tick boundary, plus a
`cycles_count` far too high to legitimately complete in the ~333 ticks available) — this pays
a real ~1s wall-clock cost rather than using `#[tokio::test(start_paused = true)]`, because
pausing tokio's clock also fast-forwards the real `SqlitePool`'s own internal connection-
acquire timeout, turning every query into a spurious `PoolTimedOut` error; `tests/
ctrlc_abort.rs` already accepts a similar real-time cost for the same underlying reason (an
actual signal/timeout has to actually elapse). `build_loop_config_rejects_an_out_of_range_
relay_amp_before_any_driver_or_db_io` mirrors the same "no I/O before the fail-fast check"
pattern now also proven for `--write-pid`/`--yes` (`run_rejects_write_pid_without_yes_before_
starting_the_tune`).

### Live-plant safety hardening

A post-`cli-logging` review of the live-tuning path (`commands/tune.rs`) surfaced nine
findings before the CLI's first real trial against live plant equipment: Ctrl+C/timeout
cancellation not reaching an in-flight driver call, no guaranteed restore on every exit
path, missing input validation (e.g. `--cycles-count 0` panicked mid-run), OPC quality never
checked, PID write-back with no pre-read/rollback, `bhtune-db`'s `restore_from` unsafe under
an active WAL and wrong on Windows, `--output json` emitting prose ahead of the JSON object,
and no template/tag snapshot on a recorded run. All nine are closed, each landed as its own
commit with its own test coverage; the per-finding writeups below are the permanent record
of what changed and why, kept rather than trimmed once "done" since they're the design
rationale for code that still exists (not a changelog of the review itself):

- **OPC DA MV actuation verification** — done. An accepted OPC DA MV write is only a gateway
  acceptance, not proof that the DCS/PLC reached the requested value. Every accepted relay
  command is tracked as pending and checked against its exact target before a replacement relay
  can overwrite it. The first check is scheduled at the earlier of the switch tick plus the
  process's noise-protection interval and the fixed internal
  `MV_ACTUATION_CONFIRMATION_SECS = 4` deadline; a finite readback must be within the derived
  tolerance, and a matching readback that completes after the deadline is still a failure.
  The fresh deadline read has an independent one-second bound instead of inheriting the full
  per-operation timeout, so a stalled MV read cannot hold the run open indefinitely.
  The normal OPC DA poll requests PV and MV together while a relay is pending, evaluates the MV
  evidence before persisting the PV sample or advancing the engine, and maps returned values by
  tag identity rather than response position. A separately bounded MV-only read remains as a
  fallback when polling supplies no usable evidence before the deadline. Verification is given
  priority when its deadline and a PV poll become ready together, so a due safety check cannot be
  hidden behind another engine step. An unconfirmed relay aborts without issuing the replacement
  write, records `TuneOutcome::ActuationFailed`/exit code `7` when restore is confirmed, and
  preserves the existing restore-incomplete exit code `6` when restoration is not confirmed.
  Simulator/replay runs deliberately do not perform this live-I/O verification or add timing work.

  The same tracker records one durable `tune_mv_actuations` row per accepted relay or restore
  command, including target, tolerance, deadline, attempts, readback quality, and terminal
  status. Restore remains authoritative for the original MV and is coordinated with the final
  MRFT snapback so the same target is not subjected to duplicate waits. MV observations are not
  additional PV samples: commanded MV remains the trend/sample/export series, while measured MV
  evidence is exposed only in the actuation audit. A shared PV/MV request can contribute to both
  the PV-read and MV-verification latency categories; those diagnostics overlap and must not be
  summed as independent network durations.

- **`--dry-run` removed entirely** — done. It was documented as never touching the DCS, but
  actually forced the full mode transition and stroked the MV through a complete relay test,
  skipping only the final PID write — indistinguishable from a real test for every purpose
  except the last write. No rescoped/renamed replacement was added: "runs a real relay test
  but skips one write" is already exactly what omitting `--write-pid` does in non-interactive
  mode. A genuinely non-mutating rehearsal (validate tags/template/ranges/connectivity with
  no loop I/O at all) remains on the roadmap as a separate future command, not a flag on a
  live tune.
- **No externally supplied number reaches the engine unvalidated** — done
  (`bhtune-core::range`, `LoopConfig::validate`, `bhtune-cli::args` value parsers,
  `bhtune-runtime::tune::validate_initial_state`). Previously `--cycles-count 0` reached
  `tuning_math::measure_oscillation`'s internal `assert!` and panicked _after_ the loop had
  already been switched to manual and stroked through a full relay test, with no restore on
  the panic path; ranges read from the driver or passed as flags were never checked for
  finiteness or ordering, so a `NaN` parsed by `f32::from_str` (which silently accepts the
  literal strings `"nan"`/`"inf"`) could flow into the tuning math and, ultimately, a PID
  write. Closed in four layers:
  - `bhtune_core::range` — new `PvRange`/`MvRange` validated newtypes, each with a
    `::new(high, low) -> Result<Self, RangeError>` constructor. `PvRange` only requires the
    bounds be distinct (it is used purely as a span magnitude); `MvRange` requires strict
    `low < high`, since `mrft::clamp_relay_amplitude`'s boundary math assumes that
    orientation. Both types keep their fields `pub` (unvalidated construction via a struct
    literal is still possible in-crate) — validation is enforced only at the
    untrusted-input boundary (`::new()`), not universally.
  - `LoopConfig::validate()` — extended to reject `cycles_count < 1` and
    `mrft_delay_secs > MRFT_DELAY_SECS_MAX` (3,600s, matching the built-in
    `[tuning].timeout_secs`),
    alongside the existing relay-amplitude check.
  - `bhtune-cli::args` — `finite_f32`/`positive_u32`/`positive_u64` clap `value_parser`
    functions applied to every numeric flag on `TuneArgs`/`SimulateArgs` that reaches the
    engine (relay amp, cycles count, the PV/MV range bounds, the simulator's process
    parameters, poll interval, timeout). Rejects `NaN`/infinite/zero/negative input with a
    clear message before any I/O. Deliberately _not_ applied to `mrft_delay`, `cycles_skip`,
    `noise_protection_secs`, or `sim_seed` — each is either bounded only at the model level
    or has no invalid range at the CLI layer (`0` is a legitimate RNG seed).
  - `bhtune-runtime::tune::validate_initial_state` — a checkpoint between
    `read_initial_values` and `transition_to_manual` (the single choke point before any
    mutation of the live loop) that validates the resolved `InitialState` uniformly,
    regardless of whether each value came from a CLI flag or a driver tag: constructs
    `PvRange`/`MvRange` from the read ranges and confirms the initial MV falls inside the
    validated MV range. `read_f32`/`resolve_f32` additionally reject non-finite parsed
    values directly, closing the `"nan"`/`"inf"` string-parsing gap before a value is even
    assembled into `InitialState`.

  An `execute()`-level integration test proves the actual safety property end-to-end: a
  driver reporting an inverted MV range (`low >= high`) causes `execute` to fail with no
  entries at all in the driver's write log — i.e. `transition_to_manual` never runs, not
  merely "the tuning math never runs".

- **Every run now snapshots the template it was configured against, not just its name** —
  done. `tune_runs` recorded no template or tag information at all, so a historical run
  could not be reinterpreted once the template catalog underneath it changed — a real
  concern given Phase 6.6 makes catalog edits routine. `tune_runs` gained four columns:
  `template_name`/`template_origin` (flat, filterable — the same denormalized-for-`WHERE`
  precedent as the existing `loop_name` column) plus `template_snapshot_json`/`tags_json`
  (the full serialized `DcsTemplate`/`LoopTags`, `CHECK (json_valid(...))`-constrained, for
  exact reproduction even after the template type itself gains fields). No foreign key to
  `dcs_templates`: a run must stay interpretable even if that row is later renamed or
  deleted. `bhtune_db::models::TemplateOrigin` (`Builtin`/`Catalog`/`User`) captures where a
  template came from; `TuneRunRow::start` now takes `template_origin`/`template: &DcsTemplate`/
  `tags: &LoopTags` alongside the existing `config`, serializing the latter two with
  `.expect(...)` on the same "infallible because upstream validation already guarantees
  every `f32` is finite" basis as `enum_to_text`. A new `DbError::InvalidJsonShape` variant
  covers the case where a stored blob is syntactically valid JSON (guaranteed by the schema)
  but no longer deserializes into the current `DcsTemplate`/`LoopTags` shape. `bhtune history
show` (not `list`, to keep the list view narrow) prints the snapshotted template name and
  origin alongside the run's other identity fields, from `RunDetailJson`/the plain-text
  table.
- **OPC quality now enforced on every tuning-critical read** — done
  (`bhtune-runtime::tune::check_quality`, `bhtune_db::models::SampleQuality`). Previously
  `bhtune_driver::Quality`/`is_trustworthy()` existed but nothing in the tune path ever
  called it — a tag reporting `Uncertain` (a stale held-last-value during a comms hiccup) or
  outright `Bad` quality flowed into the MRFT engine and a PID write-back exactly like a
  trustworthy `Good` reading. `check_quality` is now the single choke point: `Good` always
  passes; `Bad` is never accepted; `Uncertain` is accepted when the global
  `allow_uncertain_quality` policy is enabled (the default), logging a loud
  `tracing::warn!` every time so a run executed under the relaxed policy is never silently
  indistinguishable from a normal one. The policy is configured in TOML or on the browser's
  Config page, not per tune. Wired
  through every read that feeds a tuning decision:
  - `read_initial_values`/`transition_to_manual`'s setpoint read — a poor-quality reading
    before any mutation of the loop is a hard failure (a plain `anyhow::Error`), since
    nothing has been mutated yet and there is no loop state to restore.
  - The in-flight MRFT poll loop (`run_polling_loop`) — a poor-quality PV sample here _does_
    abort the run (a new `AbortReason::PoorQuality { tag, quality }`, restored and recorded
    exactly like a Ctrl+C/timeout abort), but the triggering sample is still recorded to
    `tune_samples` (with its real, poor quality) _before_ the abort, via a new
    `read_pv_sample` helper that returns quality without hard-failing on it, so the future
    history explorer can show exactly what was seen when the run gave up.
  - The PID write-back confirmation readback (`maybe_write_back`) — a poor-quality readback
    is classified as a write-back failure (`WriteBackOutcome::Failed`, audited via
    `TuneWriteRow`), never silently accepted as proof the write landed. A poor-quality
    readback also drives finding 6's rollback of whatever was already confirmed, below.

  `tune_samples` gained a `pv_quality` column (`SampleQuality`: `Good`/`Uncertain`/`Bad`, the
  DB-side mirror of `bhtune_driver::Quality` — two separate enums since `bhtune-driver` and
  `bhtune-db` are sibling crates, neither depending on the other) and `tune_runs` gained
  `allow_uncertain_quality`, so a run's quality posture is part of its permanent history.
  A poor-quality abort exits with `EXIT_POOR_QUALITY` (5), distinct from a Ctrl+C/timeout
  abort, and `--output json` carries nullable `poor_quality_tag`/`poor_quality` fields
  alongside the existing `timeout_secs`.

- **Ctrl+C and the global `[tuning]` timeouts reach an in-flight driver call, and the restore
  itself is bounded** (`bhtune-runtime::cancel`, `bhtune-runtime::tune::{bounded_driver_call,
attempt_restore}`). Previously the signal listener and the timeout sleep were both
  reconstructed fresh on every polling-loop iteration, inline in a `tokio::select!` — so for
  the entire duration of a tick's body (the PV read, the relay MV write, the sample insert)
  neither existed, and a Ctrl+C delivered in that window was silently lost (tokio coalesces
  signal delivery per kind, and a `Signal` future created _after_ delivery never observes
  it), with no fallback to the OS's default terminate-on-SIGINT behavior either (tokio
  replaces it process-wide the first time `ctrl_c()` is ever polled, and never reverts it). A
  hung driver read made the loop uninterruptible outright — exactly the scenario the global
  timeout settings are intended to prevent, and the very claim ("fires even mid-hung-read")
  that this fix makes true rather than aspirational. Closed in three parts:
  - `bhtune_runtime::cancel::CtrlC` — one process-wide Ctrl+C listener, installed exactly once at
    CLI startup (`CtrlC::install`, never from a function unit tests exercise) and threaded
    explicitly through `execute`/`run_polling_loop`/
    `attempt_restore` as `&mut CtrlC` rather than each calling `tokio::signal::ctrl_c()`
    itself. Built on `tokio::sync::watch` (not `tokio_util::sync::CancellationToken`, which
    would add a dependency) specifically for its per-clone "have I observed this value yet"
    semantics: `CtrlC::signalled()` resolves immediately for a signal that arrived at any
    point before that call — including before the handle's first call at all — and a
    _second_ signal is a second, distinguishable resolution on the same handle, which is
    exactly the "first Ctrl+C aborts, second forces a hard stop" distinction below needs.
    `CtrlC::never()`/`CtrlC::test_pair()` back runtime's direct-call tests; the CLI uses an
    installed handle and the server uses a manually-triggered handle. Unit tests never install
    a real process-wide signal handler (which would
    otherwise risk swallowing a developer's own Ctrl+C to a hung `cargo test`).
  - `bounded_driver_call`/`TickOperation` — races one driver call (the tick's PV read, or
    its MV write) against `ctrl_c.signalled()` and a fresh `[tuning].op_timeout_secs` sleep
    (default 30s, capping a single operation rather than the whole run), returning
    `Completed(T)`/`Cancelled`/`TimedOut`; a genuine `Err` from the call itself still
    propagates via `?` rather than being folded into this enum, since a rejected write or a
    transport error is a real failure, not "gave up waiting". `run_polling_loop`'s outer
    `tokio::select!` (covering the _idle_ wait between ticks) reuses the exact same `&mut
CtrlC` handle passed down into the tick body's `bounded_driver_call`s, which is safe
    specifically because a tokio `watch::Receiver`'s "seen this value" state advances the
    moment either `select!` observes it — there is no way for the outer and an inner
    `select!` to each separately consume the same signal.
  - `attempt_restore`/`RestoreAttempt` — wraps `restore()` in the same race, against
    `[tuning].restore_timeout_secs` (default 30s, independent of `[tuning].op_timeout_secs`/
    `[tuning].timeout_secs`, since a restore triggered _by_ a timeout would otherwise inherit an
    already-expired budget) and `ctrl_c.signalled()` again — a _second_ Ctrl+C during the
    restore is what "forces a hard stop" means in practice, since the restore is the one
    thing that keeps running after the first signal aborts polling. The configured timeout is
    the restore's initial absolute budget: once the authoritative MV restore write is accepted,
    the effective deadline is extended to at least `accepted_at + MV_ACTUATION_CONFIRMATION_SECS`
    so the mandatory four-second confirmation window cannot be truncated by the initial budget.
    The inner MV verification race and the outer remaining-restore race both use that mutable
    effective deadline.
    `RestoreAttempt::Incomplete { reason }` (timeout or second-Ctrl+C, distinguished only by
    `reason`'s text) prints an operator-facing `eprintln!` naming the MV tag and its
    pre-test value plus a structured `tracing::error!`, and becomes a new
    `RunOutcome::RestoreIncomplete`/`TuneOutcome::RestoreIncomplete`, exiting
    `EXIT_RESTORE_INCOMPLETE` (6) — distinct from `EXIT_ABORTED` (2), since "aborted and
    restored" and "aborted, restore abandoned, go check the loop by hand" are very different
    outcomes for a scheduler to alert on.

  **Testing approach.** A `MockDriver.hanging_read`/`hanging_write` (awaits
  `std::future::pending::<()>()` before ever reaching its own bookkeeping, so a hung call is
  provably never recorded even though the abandoned future is only dropped, not signalled)
  backs two new `run_polling_loop` integration tests: a stalled PV read aborting via the
  configured operation timeout with no sample recorded (no valid tick exists yet), and a stalled MV
  write being cancelled by a `CtrlC::test_pair()`-driven background task (standing in for a
  human pressing Ctrl+C mid-write) while still recording the sample from that tick's earlier,
  already-completed PV read. `bounded_driver_call`/`attempt_restore` also each have direct
  unit tests exercising all of their outcomes in isolation (completed/cancelled/timed-out/a
  genuine error propagating; confirmed/incomplete-via-timeout/incomplete-via-second-Ctrl+C),
  and one `run_with_ctrl_c` test exercises the real (non-test-only) entry point end-to-end
  with a simulated signal, rather than only through the `CtrlC::never()`-backed `run` every
  other test in the module uses. A real-time delayed-write/delayed-read regression test also
  proves that an MV restore accepted near the initial deadline still receives its complete
  confirmation window and that the remaining restore steps are allowed to finish.

- **Every exit path now funnels through one best-effort, all-steps-attempted restore** — done
  (`bhtune-runtime::tune::{MutationGuard, RestoreReport, RestoreStepOutcome, restore, execute}`,
  `bhtune_db::models::{RestoreStatus, TuneRunRow::record_restore_status}`). Previously
  `execute()` could transition a loop to manual and then return without ever calling
  `restore()` at all — any `?` between the transition and the polling loop (the
  `record_initial_readings` DB write, engine construction), or a `persist_results`/`complete`
  failure _after_ a genuinely completed test — and `restore()` itself returned on its first
  failure, so a single rejected MV write pre-empted even _attempting_ to put the mode back.
  Closed in three parts, matching the design's "A + C + D" decision:
  - **`MutationGuard`** (Option A) — a plain struct of four booleans
    (`mode_attribute_written`/`mode_written`/`mv_written`/tracks whether a setpoint was
    captured), armed the instant each corresponding write actually succeeds, never
    optimistically before. `execute()`'s mutating body was split into an inner function
    returning `Result<_, (anyhow::Error, MutationGuard)>` — the guard travels _with_ the
    error on every failure path — so the outer function can unconditionally consume
    whatever guard state exists (fully armed, partially armed, or the zero value from a
    failure before any write) and call `restore()` accordingly on every single exit, with no
    path that skips it. Nothing is ever "restored" that the guard doesn't say was actually
    changed.
  - **`RestoreReport`/`RestoreStepOutcome`** (Option C) — `restore()` now attempts all four
    steps (MV, mode, setpoint, mode attribute) unconditionally rather than returning on the
    first `Err`, collecting each step's own `RestoreStepOutcome`
    (`NotNeeded`/`Succeeded`/`Failed(String)`). The MV step is never gated by the guard (a
    relay-stroked MV always gets written back, since nothing else in the guard implies it
    wasn't touched); the mode/setpoint/mode-attribute steps are each gated by their own
    guard flag _and_ a value-based precondition (e.g. the mode-attribute step only fires if
    the read-back program value actually differs from what's already there), so a step whose
    guard flag was never armed correctly reports `NotNeeded`, distinct from an armed-but-
    failed `Failed`. `RestoreReport::failure_summary()` names every failed step by label
    (`"MV: ...; mode: ...; setpoint: ...; mode attribute: ..."`) rather than collapsing to
    "something failed", so an operator reading `bhtune history show` knows exactly what to
    check by hand.
  - **Durable restore intent** (Option D, partially done) — `TuneRunRow::record_initial_readings`
    now persists `mode_raw`/`mode_attribute_raw`/`setpoint_ini` (the loop's pre-mutation
    mode/mode-attribute/setpoint, mirroring the existing `pv_ini`/`mv_ini`/range columns)
    _before_ `transition_to_manual`'s first write, not after — so a process that dies
    outright (SIGKILL, power loss, a second Ctrl+C during an already-incomplete restore) still
    leaves a durable, reconstructable record of what needs to be put back, not just an
    in-memory `MutationGuard` that dies with the process. New `restore_status`
    (`RestoreStatus::Confirmed`/`Incomplete`) and `restore_detail` columns on `tune_runs`
    record the outcome of the post-run restore attempt itself (`None` means no restore was
    ever attempted — either nothing was mutated, or the run is still in progress), surfaced
    in `bhtune history show`'s table and JSON output. **Not yet done:** the
    `bhtune restore-loop --run <id>` replay command the design calls for, to actually act on
    that persisted intent later. Deliberately deferred — finding 6's own "read historical
    values, write them back under a confirmation gate" command,
    `bhtune history revert <run-id>`, is now implemented (see below), and shares enough
    shape with a future `restore-loop` that it is worth revisiting whether the two should
    share code once `restore-loop` is actually built, rather than assuming up front.

  **Testing approach.** A direct unit test on `restore()` (bypassing `execute()` entirely,
  via a hand-constructed fully-armed `MutationGuard` and a driver where all four writes
  fail) proves every step is attempted independently and the summary names all four. Three
  `execute()`-level integration tests cover the guard's actual exit paths:
  `transition_to_manual` failing on its very first write (before the mode-revert path is
  ever armed) still runs the unconditional MV restore step, leaves `MODE` untouched, and
  records `Incomplete` with a "mode attribute" detail; a `persist_results`/`complete`
  failure after a genuinely completed simulator test still attempts the restore and records
  `Confirmed`; and a poor-quality abort partway through polling (via a new
  `MockDriver::degrade_quality_after` test-harness extension, returning a tag's quality as
  `Good` for the first N reads and a chosen `Quality` after) still runs the restore end to
  end and records `Incomplete`.

- **PID write-back now pre-reads, verifies against tolerance, and rolls back a partial
  write** — done, core rewrite (`bhtune-runtime::tune::{read_previous_pid_values,
pid_value_within_tolerance, write_and_verify_pid_value, rollback_pid_writes,
maybe_write_back}`, `bhtune_db::models::{NewTuneWrite, RollbackState}`). Previously the
  three constants were written in sequence with no pre-read at all: if P succeeded and I was
  rejected, the loop was left with a mismatched, half-updated set and no way to know what P
  used to be. A transport error during the confirmation readback propagated via `?` and
  skipped the audit row entirely — the single most alarming failure mode was the one least
  likely to be recorded. "Confirmation" itself only checked that three values parsed as
  numbers, without checking quality or how close they were to what was requested, so a
  clamped or stale readback was indistinguishable from genuine confirmation. Closed as:
  - **Pre-read is a hard stop.** `read_previous_pid_values` reads P, then I, then D
    (subject to finding 5's quality rule), failing on the first bad read before anything is
    written — the run's `previous` values are always fully known or the write never starts,
    so a rollback target always exists once anything has been written.
  - **Write, verify, and check tolerance, one constant at a time.** `write_and_verify_pid_value`
    reuses `write_value` (so a transport error and a rejected write both surface the same
    way, rather than a raw `?` skipping the audit row) and `read_f32` (so a poor-quality
    confirmation read is never mistaken for success), then checks the readback against
    `pid_value_within_tolerance` — a combined absolute (`1e-3`) and relative (1%) tolerance,
    rather than exact equality, since a DCS's own unit conversion means a just-written float
    is not guaranteed to read back bit-identical, and a purely relative tolerance breaks
    down for a requested value at or near zero (e.g. `D = 0` on a PI controller).
    `maybe_write_back` calls this once per constant, in P/I/D order, stopping at the first
    failure — implemented as a loop over a fixed 3-element array of `(label, tag, requested,
previous)` tuples with index-based `[Option<f32>; 3]` temporaries for the written/readback
    values, rather than string-matching on `label`, then unpacked into `NewTuneWrite`'s named
    fields once the loop ends.
  - **Roll back only what was actually confirmed.** A constant is only added to
    `rollback_targets` after its own write-and-verify succeeds, so if P succeeds and I fails,
    only P is rolled back — D, never attempted, needs no rollback and I, never confirmed,
    has nothing to put back either. `rollback_pid_writes` mirrors `restore()`'s "attempt
    every step independently" philosophy from the previous bullet rather than stopping at
    the first rollback failure, collecting every failure so a rollback that only partially
    succeeds is still fully reported.
  - **Four distinguishable outcomes**, not just success/failure: wrote nothing (pre-read
    failed, `previous = None`, `rollback_state = None`); wrote and confirmed everything
    (`success = true`); wrote some, failed, rolled back successfully
    (`rollback_state = Succeeded`); and wrote some, failed, and the rollback _itself_ failed
    (`rollback_state = Failed`, `rollback_error` set) — printing a message pointing the
    operator at `bhtune history revert <run-id>` for the last case, since the loop may now
    hold a mismatched set of constants with no automated way left to fix it.
  - **`tune_writes` gained five columns**: `proportional_previous`/`integral_previous`/
    `derivative_previous` (nullable — the pre-read itself can fail) and `rollback_state`
    (`CHECK` constrained to `succeeded`/`failed`, `NULL` meaning rollback was never needed)/
    `rollback_error`. The existing `proportional_written`/`integral_written`/
    `derivative_written` columns were relaxed from `NOT NULL` to nullable, since a partial
    write can now leave a later constant's `written`/`readback` genuinely absent rather than
    forced to some placeholder value — added to the one pre-release migration in place,
    since nothing has shipped yet.

  **Testing approach.** `MockDriver` gained three more builders alongside the existing
  `degrade_quality_after`: `erroring_read_after`/`rejecting_write_after` (a tag's first N
  reads/writes succeed normally, then every one after that fails — letting a test put a
  tag's _pre-read_ in good standing while still forcing its _post-write_ readback or a later
  _rollback_ write to fail deterministically) and `distorting_write` (silently perturbs a
  written float by a fixed offset before storing it, so a readback that parses fine and
  reports `Good` quality can still be exercised as an out-of-tolerance rejection — a failure
  mode distinct from an erroring or poor-quality readback that no prior mock capability could
  produce). Dedicated tests cover all nine `maybe_write_back` outcomes: a full success
  (asserting `previous`/all three `*_written`/`*_readback` fields and `rollback_state = None`
  together, not just the top-level `WriteBackOutcome`), a pre-read failure (`previous = None`,
  nothing on the driver's write log at all), a rejected write, a readback that errors after
  the pre-read has already succeeded, a poor-quality readback (distinguished by message
  prefix from both the read-error case and a pre-read failure), an out-of-tolerance readback,
  a successful rollback (confirming the driver's live value was actually restored, not just
  the audit row), a rollback that itself fails (`rollback_state = Failed` with
  `rollback_error` naming the constant), and an `Uncertain` readback accepted without
  incident when the global `allow_uncertain_quality` policy is enabled (proving finding 5's
  configurable policy and finding 6's tolerance check compose correctly rather than the latter
  accidentally re-imposing a `Good`-only rule of its own). `pid_value_within_tolerance` also
  has direct unit
  tests pinning down its exact-match, relative-band, absolute-floor-near-zero, and
  negative-value behavior.

- **`bhtune history revert <run-id>` — done** (`commands::history::revert`,
  `bhtune_db::models::WriteKind`). Undoes a past PID write-back by writing the run's
  recorded pre-write values back to the live loop, so a write-back that turns out to have
  been wrong can be corrected days later without anyone having written the old numbers down
  by hand.
  - **`tune_writes` gained a `kind` discriminant** (`WriteKind::Write`/`Revert`, `CHECK`
    constrained, defaulting to `Write` in `NewTuneWrite::new`) rather than a new table — a
    revert is structurally identical to a write-back (same pre-read/write/verify/audit
    shape), just run against a historical target instead of a freshly calculated one, and
    `tune_writes` has no `UNIQUE (run_id, response_level)` constraint blocking a second row
    at the same response level. `history show`'s write-back audit listing now prints each
    row's kind alongside its response level (`Write (Moderate level)` /
    `Revert (Moderate level)`) so a revert is never mistaken for the original write.
  - **Validates before ever connecting to the driver.** In order: the run exists; the run
    used the `Opcda` driver (a `Simulator`/`Replay` run has no live loop to revert against);
    the run has at least one recorded `Write`-kind row (nothing to revert otherwise); that
    row's `previous` is `Some` (a write whose own pre-read failed has nothing recorded to
    revert to); `--yes` was passed (reverting writes to a live loop, same confirmation gate
    as the original write-back); the run's snapshotted tags have all three PID constant tags
    configured. Only after all six checks pass does it call `OpcDaDriver::connect` — so five
    of these checks are exercised in tests with no mock driver running at all, and even the
    connection-failure path itself is a genuine test (an unreachable host, proving every
    earlier check passed).
  - **Uses the run's own recorded connection — never re-resolves one** (`db-run-request-snapshot`,
    `resolve_revert_connection`). This closes a real latent safety bug: reverting used to
    resolve `--bridge-host`/`--server` from the flag/config precedence chain _at revert
    time_, so a tune run against `Kepware.KEPServerEX.V6` on gateway A could be reverted
    from a shell whose config pointed at gateway B — silently writing the first loop's old
    PID constants into a _different plant's_ controller, using tag names that may well exist
    on both. Now `resolve_revert_connection` always trusts `run.opc_server`/`run.bridge_host`
    (populated by `TuneRunRow::record_connection` when the original run started, and a hard
    error if either is missing — an old run predating this feature, or a `Simulator`/`Replay`
    run that never recorded one); an explicit `--server`/`--bridge-host` flag is only a
    cross-check, and a hard error if it contradicts the stored value rather than silently
    overriding it. `--bridge-host` deliberately has no `BHTUNE_BRIDGE_HOST` env fallback the
    way every other command's `--bridge-host` does, precisely so an unrelated ambient env var
    can never itself trigger a false "contradicts the recorded one" error.
  - **Reuses `bhtune-runtime::tune`'s own pre-read/write-and-verify helpers directly**
    (`read_previous_pid_values`, `write_and_verify_pid_value`, promoted from private to
    `pub(crate)` for this purpose) rather than re-implementing them, so a revert's pre-read,
    tolerance check, and per-constant failure semantics are identical to the original
    write-back's by construction, not by parallel maintenance. Like the original write-back,
    a revert pre-reads the loop's _current_ live values first and records them as its own
    `previous` — so a revert that turns out to be wrong can itself be undone by reverting
    again — then writes and verifies Proportional, Integral, and Derivative in order,
    stopping at the first failure. A revert never chains a nested rollback of itself
    (`rollback_state` stays `None` on every revert row); a partially-failed revert is
    reported and audited, matching a partially-failed original write-back's "roll back only
    what was confirmed" philosophy being a deliberately separate concern from "undo an old
    write-back on request".
  - **Format-aware reporting**, matching the "Option B" design used for the original
    write-back: a `Table`-mode status line before attempting the revert and a plain-text
    summary after; a `RevertJson`/`RevertedTargetJson` object (run id, response level, the
    target P/I/D values, success, and an error message) on `--output json`, with no prose
    printed ahead of it.

  **Testing approach.** Thirteen dedicated tests (`db-run-request-snapshot` added three:
  `revert_errors_when_the_run_has_no_recorded_connection`,
  `revert_errors_when_an_explicit_server_flag_contradicts_the_recorded_one`, and
  `revert_errors_when_an_explicit_bridge_host_flag_contradicts_the_recorded_one`). Ten need
  no mock driver at all, since `revert`'s validation runs before it ever connects: no such
  run; the run used a non-`Opcda` driver; no `Write`-kind row recorded; the recorded write's
  `previous` is `None`; `--yes` not passed; the run's tags have no PID constant tags
  configured; the run has no recorded connection at all; an explicit `--server`/
  `--bridge-host` that contradicts the recorded one (one test each); and a genuine connection
  failure using the run's own recorded (but unreachable) connection, proving every check
  above passed. Two use the shared mock gRPC `Bridge` service from `crate::test_support`: a
  full success
  (asserting the resulting row's `kind = Revert`, all three written/readback values, and
  `rollback_state = None`), and a partial failure using the mock's existing
  `failing_read_from_call(n)` builder to fail exactly Integral's post-write verification
  readback (call 5 of 6: three pre-reads plus Proportional's own verification succeed
  first), proving Derivative is never attempted and the failure is still fully audited. A
  final test exercises the `--output json` success path directly (`bhtune-cli`'s own
  subprocess-level "stdout is exactly one JSON object" contract remains
  `safety-json-contract`'s responsibility, not re-proven per command here).

- **`bhtune-db`'s `restore_from` is now safe under an active WAL and requires exclusive
  access before restoring** — done (`bhtune_db::backup::exclusive_pre_restore_snapshot`,
  `EXCLUSIVITY_PROBE_TIMEOUT`, `DbError::DatabaseInUse`). This finding predates any CLI
  command actually calling `restore_from`/`backup_to` (both remain library-only APIs, per
  `db-backup-restore`) — genuinely proactive hardening rather than a fix to a shipping
  path, but sequenced here rather than deferred since Phase 6.6's template catalog work
  edits the same pre-release migration findings 6 and 9 already touch. Previously the
  pre-restore safety copy used a raw `std::fs::copy`, and SQLite only auto-checkpoints a
  WAL when the _actual last connection to the file across the whole system_ closes — not
  merely the last connection in the caller's own pool — so a copy taken while a second
  process (`bhtune-server` running alongside the CLI, the exact topology this project's own
  architecture anticipates) still held the database open could silently miss committed data
  sitting only in the WAL. Separately investigated and found to be a non-issue:
  `restore_from`'s existing copy-to-temp-then-`rename` file replacement was already correct
  on Windows — `std::fs::rename` overwriting an existing destination _file_ (as opposed to a
  directory) has always worked there via `MOVEFILE_REPLACE_EXISTING`, so no
  Windows-specific fallback was needed for that part of the original finding. Closed as the
  design's "A + C":
  - **`VACUUM INTO` replaces the raw copy** (Option A) — the pre-restore safety copy is now
    taken the same way `backup_to` already takes its own snapshots: a consistent,
    WAL-content-inclusive copy that can never be silently missing committed data.
  - **An exclusivity probe gates the snapshot** (Option C) —
    `exclusive_pre_restore_snapshot` opens a dedicated, single-connection probe pool
    straight to `db_path` (deliberately not via `connect()`, which runs migrations that must
    never touch a database about to be discarded) and runs `PRAGMA wal_checkpoint(TRUNCATE)`;
    a nonzero `busy` column is SQLite's own native proof that some other connection — in this
    process or any other — is still attached, with no lock-file or advisory-lock scheme
    needed to get that answer. `busy != 0` fails the whole restore with
    `DbError::DatabaseInUse(db_path)` before the snapshot or the live file are touched at
    all. A dedicated `EXCLUSIVITY_PROBE_TIMEOUT` (200ms) backs this check rather than
    reusing `pool::connect`'s general-purpose 10-second `BUSY_TIMEOUT`: the two timeouts
    answer different questions (connect's, "let contended work finish"; this one's, "is
    anyone here right now") and reusing the longer one was measured to make every
    blocked-restore path take a real ~10 seconds, since `wal_checkpoint(TRUNCATE)`
    internally retries for the full busy-timeout duration before ever reporting `busy = 1`.
  - **The residual race is accepted and documented, not hidden.** The exclusivity probe is a
    point-in-time check, not a held lock — a different process could still open `db_path` in
    the instant between the probe succeeding and the later file replacement.
    `restore_from`'s doc comment calls this out explicitly as "the honest fix for the
    multi-process case," matching the design's own framing of Option C versus the fuller,
    explicitly deferred Option D (a logical/online restore that needs no exclusivity at
    all).

  **Testing approach.** Two new tests prove the exclusivity check itself: one opens a second
  real connection to the live database, begins a transaction, and executes an actual
  `SELECT` (establishing a genuine WAL read snapshot — a bare `BEGIN` alone wouldn't hold the
  file open the same way), then asserts `restore_from` returns `DbError::DatabaseInUse` with
  the live database left completely untouched; the other confirms a retry succeeds once that
  blocking connection is dropped and closed. A third test targets the one line the new
  exclusivity step's own side effect made harder to reach: because
  `exclusive_pre_restore_snapshot`'s own open-checkpoint-close sequence against an
  _existing_ `db_path` already tidies up any stale `-wal`/`-shm` sidecars itself, the
  pre-existing orphaned-sidecar test no longer exercises the later post-rename cleanup
  loop's own `remove_file` call (caught by `cargo llvm-cov`'s line-level report, not by a
  failing test — its own assertions, checking only final restored data, still passed). The
  new test constructs the one scenario that _can_ only be cleaned up by that loop: `db_path`
  itself never existing (so the exclusivity/snapshot step is skipped entirely, per its own
  existence gate) while stale sidecar files exist anyway at the paths it would use.

- **`--output json` now emits exactly one parseable JSON value on stdout on every `tune`
  path** — done (`bhtune-runtime::tune::maybe_write_back`, `TuneOutcome::Completed`'s new
  `write_back_detail` field). Previously `maybe_write_back` `println!`ed its interactive
  listing/prompt and every status/result line unconditionally, regardless of `--output` —
  confirmed by hand: a completed simulator run (which never has PID constant tags
  configured, see `build_tags`'s `DriverKindArg::Simulator` arm) printed "No PID constant
  tags configured for this run's driver/template; skipping write-back." on stdout _before_
  the run's final JSON object, so `serde_json::from_str`/`json.loads` on stdout failed for
  every scripted/scheduled caller using `--output json` — the exact audience that flag
  exists to serve. Closed as the design's Option B ("format-aware reporting"), chosen over
  Option A (prose to stderr only) because a scripted caller ends up with strictly more
  information than today — the reason a write-back was skipped or failed is now a real,
  parseable field — rather than merely relocating prose out of the way; Option C (buffer
  the whole run and render once at the end) was rejected as a far larger refactor for a
  finding scoped to one function, with the interactive prompt still needing to print before
  reading stdin regardless.
  - **`maybe_write_back` gained an `output: OutputFormat` parameter and now returns
    `(WriteBackOutcome, Option<String>)`** instead of bare `WriteBackOutcome` — the second
    element is a human-readable detail string explaining _why_ the outcome is what it is,
    populated on every `Skipped`/`Failed` return path (no PID constant tags configured; no
    calculated results recorded; the named `--write-pid` response level has no result;
    pre-read failure; a rejected write, with or without a successful/failed rollback) and
    left `None` only for `Written` (self-explanatory) and the Table-mode "everything
    succeeded" cases exercised elsewhere. `RunOutcome::Completed`'s new `write_back_detail`
    field carries this through to `print_summary`, which folds it into the JSON object as
    `"write_back_detail"` — `Table` mode ignores it entirely (every match arm gained a
    trailing `..` to accommodate the new field without caring about its value), since the
    equivalent information is already in the `println!`ed prose there.
  - **Every remaining `println!` in `maybe_write_back` is now gated on `output ==
OutputFormat::Table`**, so `Json` mode prints nothing at all from this function — the
    caller's single final JSON object is the only thing that reaches stdout.
  - **The interactive listing/menu/prompt moved to `eprintln!` unconditionally**, in both
    output formats. A prompt has no business on stdout in _any_ format: a caller piping
    stdout elsewhere (exactly the scripted use `--output json` exists for, but just as true
    of `--output table | tee run.log`) should never see "Write which response level..."
    interleaved with the actual result.
  - **`--output json` without `--write-pid` now skips the interactive prompt outright**
    rather than attempting to read a response level from stdin — added as a new early-return
    arm (`None if output == OutputFormat::Json`) checked _before_ `reader` is touched at all,
    returning `WriteBackOutcome::Skipped` with a detail string naming the reason. There is no
    human present to answer an interactive prompt in a scripted/scheduled JSON run, and the
    prior behavior (read a line from real `stdin`, block indefinitely if none arrives) is
    exactly the kind of silent hang this project's automation posture (`cli-automation`) is
    designed to avoid. Combining `--output json` with `--write-pid <level>` still writes
    non-interactively exactly as before — this new arm only fires when no level was named.
  - **`--dry-run`'s removal (finding 1) and this finding compose cleanly**: with `--dry-run`
    gone, `--write-pid` already requires `--yes` unconditionally, so the JSON-mode
    early-exit above is reached only when a caller deliberately chose `--output json`
    without also naming a `--write-pid` level — an unusual but valid combination (e.g.
    "run the test and report the calculated constants, but never write") that now degrades
    to a clean, documented skip instead of a stdin hang.

  **Testing approach.** Six of the thirteen existing `maybe_write_back` unit tests were
  extended (rather than duplicated) with an assertion on the returned detail string, proving
  the plumbing end-to-end for each distinct skip/failure shape: no PID constant tags
  configured, no results recorded, the pre-read-failure case (asserts the detail _starts
  with_ `"pre-read failed:"`, since the underlying transport error's own message is
  interpolated), a rolled-back failure (asserts the detail _ends with_ `"(rolled back)"`), a
  failed rollback (asserts it mentions both `"rollback also failed"` and `"history
revert"`), and a named `--write-pid` level with no recorded result. One new unit test
  (`maybe_write_back_skips_the_interactive_prompt_without_touching_stdin_when_json_output_is_set_without_write_pid`)
  uses a named `Cursor` (rather than an inline temporary) specifically so `reader.position()`
  can be asserted as `0` _after_ the call — direct proof that the JSON-mode early-exit never
  reads a single byte from stdin, not just that it returns the right value. None of this,
  however, can prove the actual stdout _contract_ — `print_summary` calls `println!` directly
  and returns only a label enum, so a unit test has no way to observe the rendered JSON
  string. That gap is closed by a new subprocess-level integration test,
  `crates/bhtune-cli/tests/json_output_contract.rs`, modeled on `ctrlc_abort.rs`'s pattern of
  spawning the real compiled `bhtune` binary (`env!("CARGO_BIN_EXE_bhtune")`) rather than
  calling anything in-process: `tune_output_json_emits_exactly_one_parseable_json_value_on_stdout`
  runs a fast-completing simulator tune with `--output json`, asserts a clean exit code, and
  — the load-bearing assertion — runs `serde_json::from_str` on the _entire, trimmed_ stdout
  and asserts it succeeds, catching both "prose printed before the object" (this finding's
  original bug) and "prose printed after it"; it further asserts `write_back_detail` is a
  string containing "no PID constant tags configured" and that the old suppressed prose
  string never appears anywhere in stdout. A second test,
  `tune_output_table_is_plain_text_not_json`, is a sanity check that the default `Table`
  format for the identical run is _not_ parseable as JSON, proving the format flag actually
  branches rather than the two tests coincidentally passing the same way.
