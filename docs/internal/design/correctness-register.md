# Correctness register

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Correctness-critical design details (also the legacy bug register, `core-bug-register`)

These are easy to get subtly wrong, so they're called out explicitly. Each should have direct
unit-test coverage, not just be caught incidentally by a golden-master replay fixture. This list
also **is** `core-bug-register`'s deliverable: every legacy defect found during the migration is
covered somewhere below with an explicit replicate-or-fix decision, tagged with one of:

- **`[fixed, compat flag available]`** — the correct behavior ships by default; the old, buggy
  behavior can still be reproduced on demand via a `*Compat` struct field, for bug-for-bug replay
  against a legacy trace if one is ever needed.
- **`[fixed, no flag needed]`** — the correct behavior ships and the bug has no legitimate reason
  to ever be reproduced (it was pure defect, never a documented or relied-upon behavior).
- **`[structurally impossible]`** — stronger than "fixed": the bug's precondition cannot occur in
  the new design at all (a compile-time guarantee or a data structure that can't represent the
  invalid state), not merely "we were careful this time."
- **`[not applicable — feature dropped]`** — the subsystem the bug lived in (licensing,
  loop-locking, log encryption) was not ported at all, per the plan's locked decisions, so the
  bug has nothing to attach to.
- **`[preserved rule]`** — not a bug: a real legacy behavior that must be kept exactly as-is,
  included here because it is just as easy to accidentally break as an actual defect.
- **`[advisory diagnostic]`** — an operator-facing measurement warning that informs review but
  intentionally does not change result validity or live-loop control without field evidence.
- **`[research-only]`** — an unshipped alternative evaluated outside production behavior, requiring
  stronger closed-loop or live evidence before it can be considered for release.

1. **`[fixed, compat flag available]` The MV boundary clamp must be dimensionally consistent on
   both sides.** If the relay step
   down would drive MV below its configured floor, clamp the relay amplitude to
   `MvValueIni - MvLowerRange` (the actual distance from the initial value down to the floor), not
   an expression that adds the floor back onto the initial value. Get this wrong and cascaded
   loops with a non-zero MV floor get an incorrect (usually oversized) relay amplitude — it's
   silently masked whenever the floor is 0 (the common 0–100% case), which makes it easy to miss
   in testing. Legacy: `CheckMVboundaries`, `OPCClass.cs` ~line 355. Fixed by default in
   `core-mrft`; replicable via `MrftCompat.replicate_lower_clamp_bug`.
2. **`[fixed, compat flag available]` The MRFT oscillation period must use full-precision elapsed
   time** (total seconds as a
   floating-point value), never truncated into separate hour/minute/second integer components and
   reassembled — that discards sub-second precision and wraps incorrectly past 24 hours. This
   matters most on fast loops (flow/pressure) where the whole oscillation period is only a few
   seconds, so truncation error is a large fraction of the signal, not noise. Legacy:
   `CalculatedMRFTperiodMinutes`, `OPCClass.cs` ~line 743 (uses `TimeSpan.Seconds`, an
   integer-truncating property, instead of `.TotalSeconds`). Fixed by default in
   `core-tuning-math`; replicable via `TuningMathCompat.replicate_period_truncation_bug`.
   **Cautionary note:** `core-tuning-math`'s first implementation stated this rule correctly but
   didn't actually follow it — `measure_oscillation` computed elapsed time via
   `chrono::Duration::num_seconds()` (whole-second-truncating) _unconditionally_, with
   `TuningMathCompat.replicate_period_truncation_bug` only gating an additional 24-hour wrap on
   top of the already-truncated value. Every existing unit test used whole-second switch-time
   offsets, so this was lossless in every test and went unnoticed until `e2e-simulator`'s real,
   fixed millisecond-spaced simulator ticks through the production subprocess path hit it directly, silently zeroing `ti_minutes`/
   `td_minutes` even for PI/PID. Fixed by switching to `num_milliseconds()` for the default path;
   see the `measure_oscillation_keeps_sub_second_precision_by_default` regression test in
   `tuning_math.rs` and `e2e_simulator.rs`'s module doc for the full story. The lesson: writing
   the rule down is not sufficient on its own — it needs test coverage with genuinely
   sub-second-precision inputs, not just whole-second ones, to actually enforce it. Separately,
   `core-replay-harness`'s `flow_pi_direct` fixture needed its own dedicated, narrower
   `PERIOD_TOLERANCE_MINUTES` for exactly this reason — see "Validation strategy" above.
3. **`[structurally impossible]` Switch timestamps must reuse the already-captured tick
   timestamp**, never a fresh wall-clock
   read at the moment a switch is performed — the two can differ by however long evaluation took,
   which is small but non-deterministic and breaks exact replay comparison. Legacy:
   `MRFTperformSwitch`, `OPCClass.cs` ~line 430 (stores `DateTime.Now` instead of the tick's own
   `TimeCurrent`). Stronger than a default-off compat flag: `chrono`'s `clock`/`now` features are
   disabled workspace-wide, so `bhtune-core` cannot call `Utc::now()` even by accident — verified
   by temporarily adding such a call and confirming it fails to compile (see `driver-opcda`'s
   notes above for where this was re-verified after `opcda-bridge`/`tonic` entered the dependency
   graph). The CLI caller uses fixed-step simulator time or UTC-anchored monotonic live time;
   neither path reads the calendar clock again for an in-flight sample or switch.
4. **`[structurally impossible]` Lookup tables must be sized to exactly the number of process
   types that exist (6)** — no
   extra, unreachable rows/columns in the tuning-constant or default-cycle data. Legacy:
   `matrixCyclesSkip`/`matrixCyclesTest`/`matrixNoiseProt` each held 7 elements against only 6
   process types in the dropdown, leaving the 7th permanently unreachable. `ProcessType::ALL` is a
   `[ProcessType; 6]` and every lookup table in `constants.rs` is a plain `[T; 6]` array — the
   array length and the enum's variant count are the same 6 by construction, and
   `constants.rs`'s own tests assert `.len() == 6` on each table, so a 7th row could not silently
   exist even as an authoring mistake.
5. **`[preserved rule]` If a CSV/tabular export format is ever added**, generate the header and
   each data row's
   column order from the same single ordered list of field names — never maintain them as two
   independently hand-written strings; that's exactly the kind of thing that silently drifts out
   of sync. Legacy: Step Test's dynamic CSV log wrote the header `Time,PV,SV,MV,P,I,D` but the
   data rows as `Time,PV,MV,SV,P,I,D` — the MV and SV columns transposed. Not yet applicable to
   bhtune (Step Test is a deferred phase, per the plan's locked decisions), but recorded here so
   the eventual port doesn't repeat it; `bhtune-runtime`'s sample export serializer already
   follows the single-source-of-truth pattern this item calls for.
6. **`[fixed, no flag needed]` PID unit labels (Kp vs. PB; Ti vs. Ri vs. Ki; Td vs. Kd) must
   refresh on every relevant state
   change** — process-type change, template switch, and app startup — not only from a single
   settings-changed event handler. A partial refresh trigger is an easy way to end up with stale
   unit labels on a results screen. Legacy: `UpdateAllPIDlabels()` was only wired to the
   PropertyGrid's change handler, never called at startup or on template switch. Not applicable in
   the same form in bhtune's web frontend — React re-derives unit labels from component state on
   every render, so there is no separate "refresh" step that can be forgotten — but the underlying
   rule (labels must always reflect current process type/template, not a stale cached value) still
   held during `frontend-screens`/`template-cli` and is worth remembering if a non-React adapter
   is ever added.
7. **`[fixed, no flag needed]` Tag-name derivation from a single PV tag must use the active
   DCS/PLC template's own
   configured suffix convention, never a hardcoded literal** — different DCS/PLC families name
   their PV item differently (e.g. a `.PV` dot-suffix convention vs. no such convention at all).
   Legacy: `-t`/`--tagname` unconditionally appended the literal `".PV"`. Fixed: `bhtune-core`'s
   `derive_tag(pv_tag, suffix)` takes the suffix from the active `DcsTemplate`'s own
   `process_variable_suffix` field (and the equivalent field for every other derived tag) — the
   convention is template data, never Rust logic.
8. **`[fixed, no flag needed]` Relay amplitude needs real, enforced range validation at the
   model/construction level** — not
   just client-side keystroke filtering plus a single "not blank" check. An unvalidated numeric
   field that only rejects blanks is exactly how a nonsensical value reaches a live control loop.
   Legacy: the hidden debug codes `2014`/`2015`/`2016` typed into the Relay Amplitude box were
   validated only as "not blank", so a leftover debug code could become a 2014% relay step. Fixed:
   the debug codes were dropped entirely rather than ported (see item 10 below), and
   `safety-validation`/`LoopConfig::validate()` enforces real bounds on `relay_amp_percent` at
   construction time regardless.
9. **`[fixed, no flag needed]` Any file export feature must write to a path the user explicitly
   chooses, or a documented
   platform-standard data directory** — never an implicit hardcoded path or "wherever the process
   happened to start". Legacy: `TuningConstantsExport()` wrote to a hardcoded developer path
   (`C:\Dropbox\Auto-Tuner Proj\...`); `LogLoopLocking()` wrote to the current working directory
   rather than the log directory. Fixed: the `bhtune` CLI's `export`/`history export` commands take an
   explicit `--output <path>` (or write structured data to stdout for piping), and shared
   logging resolves its directory through the normal config precedence, defaulting to a
   documented platform-standard data directory — never an implicit/hardcoded path.
10. **`[fixed, no flag needed]` Test/demo mode must be a first-class, explicit driver choice**
    (e.g. `--driver
simulator`), never triggered implicitly by a magic tag name or hidden UI state — an implicit
    trigger is surprising and easy to leave enabled accidentally. Legacy: `OPCClass.Python` gated
    a hardcoded branch triggered by typing the magic tag name `Simulink.Device1.Python.PV`, which
    also **returned early from `ResetOPC`, skipping all DCS mode-revert logic**, and shelled out to
    a hardcoded `RunModel.bat` path while blocking the UI thread for 7 seconds. Fixed:
    `driver-simulator`'s `SimulatorDriver` is selected explicitly (`--driver simulator`), is a
    real, in-process, non-blocking FOPDT model with no shell-out, and shares the exact same
    restore/mode-revert path every other driver uses — there is no special-cased early return.
11. **`[fixed, no flag needed]` PID-type selection must be modeled as proper enums**
    (`ProportionalType`, `IntegralType`,
    `DerivativeType`, controller action direction, etc.), never as comparisons against magic
    display strings or sentinel values. Legacy: PID type selection compared against display
    strings (`"Kp - Proportional Gain"`, `"Ti - Reset Time"`, `"Ri - Reset Rate"`, `"Ki - Reset
Gain"`, `"Td - Derivative Time"`, `"Kd - Derivative Gain"`, `"Seconds"`), and a sentinel string
    `"__reverse__"` forced a mismatch against `ControllerActionDirect` when the user selected
    Reverse manually. Fixed: `core-model` ported these as proper `serde`-backed Rust enums
    (`ControllerDirection`, etc.) decoupled from any UI display label.
12. **`[preserved rule]` PID is only offered for the two Temperature process types**; every other
    process type offers
    only P and PI. This is a deliberate domain rule (rooted in which tuning-constant columns are
    actually calibrated), not an arbitrary restriction to relax. Preserved as
    `ProcessType::allows_pid()`.
13. **`[preserved rule]` Skip/count/noise-protection defaults are auto-populated per process type**
    from lookup
    tables whenever the process type changes. Preserved via `constants.rs`'s
    `DEFAULT_CYCLES_SKIP`/`DEFAULT_CYCLES_TEST`/`DEFAULT_NOISE_PROTECTION_SECS` tables (see item 4
    above for their sizing).
14. **`[preserved rule]` On the final MRFT step, MV snaps back to the initial value** rather than
    taking a full relay
    step. Preserved in `core-mrft`'s `MrftEngine::step`.
15. **`[fixed by design in this project, not a compat concern]` PID precision belongs to the
    snapshotted template, not independent frontend formatting.** Decimal places and significant
    digits have distinct semantics, especially for small gains. Pure core rounding produces
    a numeric controller target and canonical text after unit conversion; runtime previews,
    CLI review, HTTP detail, browser review, and new writes share those targets. Yokogawa uses
    one decimal place, while other built-ins and new custom templates use three significant
    digits. Active terms erased to zero are explicitly unwritable without changing raw-result
    validity. Exact disable sentinels, raw calculations/exports, previous/readback values,
    rollback, revert, and recovery retain their original precision. Run snapshots require
    explicit metadata; there is no vendor-name or historical precision fallback.
16. **`[new feature, not a legacy bug]` A live PV/MV trend chart is a core UX expectation for the
    web GUI** — plan for high-rate streaming updates (multiple times per second) from the start;
    see "Chart library" below. The legacy app never had a trend chart at all
    (`Telerik.WinControls.ChartView` was referenced in the `.csproj` but no chart control was ever
    built), so this is new scope, not parity work — shipped via `frontend-live-stream`'s
    `TrendChart` (uPlot). Short trends reserve 12 configured poll intervals before the x-axis
    switches to full elapsed-history fitting; the blank future area is intentional and no
    synthetic points are added.
17. **`[not applicable — feature dropped]` A licensing/loop-locking ledger's connection-open
    logic must handle a missing database file without throwing from an unobserved async task.**
    Legacy: `SQLock.CheckDB()` called `.Open()` on a `null` `SQLiteConnection` whenever
    `SQLock.db` was absent, so a genuinely fresh install could throw inside a fire-and-forget task
    nobody awaited. Moot: bhtune has no license-gated loop-locking ledger at all — `bhtune-db`'s
    SQLite schema (`db-schema`) was designed plain from the start (see `db-drop-legacy`), so there
    is no `SQLock`-equivalent connection-open path that could reproduce this.
18. **`[not applicable — feature dropped]` Log "encryption" and a login gate must provide genuine
    protection, or not exist at all.** Legacy: logs were "encrypted" with AES-GCM using the
    password `"imbcontrols2016"` hardcoded in the shipped binary — trivially reversible by anyone
    holding the exe — and `Login.cs` (which shared the same hardcoded password) was itself dead
    code, since `Program.cs` ran `MainForm` directly and never instantiated the login form, making
    the documented `--unlockApp` flag a literal no-op. Moot: per the plan's locked decisions,
    bhtune ships no log encryption and no login gate at all — logs and the database are plain,
    matching the "no need to obfuscate/encrypt/hide anything" requirement — so there is no
    encryption or auth subsystem left to get subtly wrong in this way.
19. **`[fixed, no flag needed]` An accepted MV write must not be treated as proof of physical
    actuation.** An OPC DA gateway can accept a write while the DCS/PLC leaves the tag unchanged,
    clamps it, updates it late, or returns a stale value. OPC DA relay and restore commands are
    therefore tracked in `tune_mv_actuations` and read back against their exact targets within a
    fixed four-second confirmation window; a matching read that completes after the deadline is
    still invalid, and a due verification runs before a due PV poll. While a relay is pending,
    normal OPC DA polling requests PV and MV together, maps the response by tag identity, and
    evaluates MV evidence before the engine advances; a bounded MV-only read remains as a fallback.
    The restore timeout is only its initial budget: an authoritative MV restore write accepted near
    that boundary extends the effective restore deadline to at least four seconds after acceptance,
    and both MV verification and the remaining restore steps share that extension. A mismatch
    aborts before a replacement relay is issued and restores the loop, with confirmed restoration
    reported as `TuneOutcome::ActuationFailed`/exit code `7` and incomplete restoration retaining
    the higher priority `RestoreIncomplete`/exit code `6`. MV evidence never becomes an additional
    trend/sample/export point or changes MRFT timing. A shared batch can populate both PV-read and
    MV-verification latency categories, which overlap and must not be summed. Simulator and replay
    drivers remain free of this live-I/O policy. The detailed actuation audit remains available
    through the CLI/API/database/logs; the normal browser run-detail page shows the summarized
    outcome instead of the raw table.
20. **`[fixed, no flag needed]` Base Tag changes must invalidate Custom mappings, and historical
    result labels must come from the run's template snapshot.** The New Run form routes manual
    edits, template-driven suffix replacement, and OPC browser selection through
    `applyTagNameChange`; it resets direct Custom tag mappings and custom direction/range read
    mappings to their template/tag sources and clears their custom values, but preserves Fixed
    value direction/range mappings and values. Request hydration treats null or omitted
    direction/range values as Template tag sources, not Fixed value sources, so Duplicate this run
    and newest-run prefill preserve the original Template/Custom/Fixed modes. The run-detail
    Calculated results table uses proportional/integral/derivative labels from the snapshotted
    template rather than the mutable catalog; it intentionally keeps all three template-specific
    columns visible for P, PI, and PID, and PI runs display derivative `0` because the write path
    explicitly clears stale derivative action.
21. **`[fixed, compat flag available]` Measurement extrema must not be conflated with hysteresis
    extrema.** The legacy reset of both trackers to `pv_value_ini` after a switch can discard the
    shared switch endpoint from the next measured half-cycle when polling is sparse, producing a
    zero or biased PV amplitude even though the relay state machine itself is behaving
    consistently. Production `MrftEngine` therefore keeps separate result extrema, includes the
    switch sample in the new measurement interval, and leaves the hysteresis trackers on the
    legacy reset so switching behavior is unchanged. `MrftCompat::replicate_extrema_reset_bug`
    is available only for parity experiments. Variant B — changing the hysteresis reset too — is
    `[research-only]` and must be evaluated with a closed-loop simulator rather than a recorded
    trace.
22. **`[fixed, no flag needed]` Degenerate calculated results must never become writable PID
    constants.** A zero/negative/non-finite PV amplitude or period, a non-finite tuning
    intermediate, or a non-finite template-converted P/I/D value produces an explicitly invalid
    response-level row with a diagnostic reason and no numeric values. `calculate_all_checked` is
    the production boundary; SQLite constraints preserve the shape, and CLI/HTTP write paths
    require a valid result before connecting to a driver. The run's raw samples and
    invalid-result evidence remain available for history investigation.
23. **`[advisory diagnostic]` Sampling and operation latency must be visible before changing
    timing defaults.** `TimingMetrics` classifies observed samples per oscillation period as
    `adequate` at `>= 6.0`, `marginal` below that, or `not_assessed` without a usable period, and
    records successful operation-latency summaries for PV reads, MV writes, MV verification reads,
    sample persistence, and total tick work. These diagnostics are intentionally advisory: they
    do not invalidate otherwise finite results, abort a run, or block a valid PID write until
    field evidence supports a stronger policy.
24. **`[fixed, no flag needed]` Saved OPC tag restoration must settle before it is shown.**
    `OpcTagBrowserModal` leaves the tree mounted so layout can be measured, but keeps the
    asynchronous root load, search fallback, breadcrumb expansion, exact selection, and inner
    viewport scroll behind a shared blocking overlay. The overlay is cleared only after the
    selected row is rendered and fully visible; root errors, unavailable tags, cancellation, and
    fallback selection must all clear it deterministically. Reuse shared loading primitives
    instead of introducing one-off spinners or text-only waits for equivalent frontend work.
