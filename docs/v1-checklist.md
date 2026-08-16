# v1 acceptance checklist

This is the acceptance-criteria baseline for v1. Every planned behavior is listed here with an
explicit disposition, so "is this done, and does it still work" has a concrete, checkable answer
instead of relying on memory or a general sense of completeness.

Check an item once it is genuinely done for that disposition (implemented and passing
golden-master replay where applicable; or deliberately deferred/not-planned and that decision is
reflected in code and docs, not just here).

## Disposition legend

- **REQUIRED** — must be implemented for v1. Where the behavior affects tuning math or logged
  output, it must pass golden-master replay against a reference trace, not just "look right". See
  the correctness-critical design details in [`AGENTS.md`](../AGENTS.md#correctness-critical-design-details)
  for implementation notes on the trickier items.
- **DEFERRED** — a real feature, genuinely out of scope for v1. Tracked on the roadmap
  (`AGENTS.md`/`README.md`), not silently forgotten.
- **NOT PLANNED** — explicitly out of scope, called out so it's clear the absence is deliberate
  rather than an oversight (e.g. no usage gating, no log encryption).

## 1. CLI / automation surface

- [ ] REQUIRED — `-t`/`--tagname` sets the PV tag prefix and triggers tag-name derivation for the
      rest of the tag set, using the active DCS/PLC template's own suffix convention (never a
      hardcoded literal — different DCS/PLC families name their PV item differently).
- [ ] REQUIRED — `-c`/`--computer` (OPC host) and `-s`/`--opc-server-id` (OPC server ProgID)
      equivalents. If a default host is documented (e.g. `localhost`), it must actually be
      implemented, not just claimed in help text.
- [ ] NOT PLANNED — any encryption toggle for logging. Nothing in BHTune is encrypted; every tune
      run is recorded to SQLite by default. A "verbose tick-level logging" flag, if wanted, is a
      normal `cli-logging` flag with its own semantics.
- [ ] REQUIRED — pre-test and post-test recording padding (an `--mrft-delay`-equivalent flag): PV
      is still read/recorded every tick during the pad; no switch evaluation happens during it.
- [ ] NOT PLANNED — any app-unlock/login-bypass flag. BHTune has no login gate to bypass.

## 2. Relay amplitude validation

- [x] REQUIRED — Relay Amplitude must have real, enforced range validation at the
      model/construction level, not just client-side keystroke filtering or a single "not blank"
      check. An unvalidated numeric field is exactly how a nonsensical value reaches a live
      control loop.

## 3. UI surface → screens

BHTune's web GUI frontend screens (`frontend-screens` phase) and equivalent CLI
subcommands/flags (`cli-commands` phase) need to cover the following capability groups. The UI
does not need matching widgets to any particular layout, only equivalent capability plus real
validation.

- [ ] REQUIRED — Loop Configuration: set/browse the PV tag; derive/fill the rest of the tag set
      from it.
- [ ] REQUIRED — Algorithm Settings: process type (6 types), controller type, relay amplitude,
      cycles to skip/test, noise protection delay.
  - [ ] REQUIRED — **P and PI are the only controller types offered for four of the six process
        types, and PID is only offered for the two Temperature types** — this is a real domain
        rule, not an oversight, and must not be "fixed" into always offering PID.
  - [ ] REQUIRED — skip/count/noise-protection defaults auto-populate per process type from
        lookup tables when process type changes.
- [ ] REQUIRED — Test Data: start/stop controls, live elapsed time, cycles completed/remaining,
      live PV/MV readout with significant-digit-aware display formatting (see §9).
- [ ] REQUIRED — Results: Aggressive/Moderate/Sluggish PID values, with dynamic unit labels
      (Kp vs. PB; Ti vs. Ri vs. Ki; Td vs. Kd) that refresh on every relevant state change
      (process-type change, template switch, app startup), and confirm-then-write actions per
      response level.
- [ ] REQUIRED — Tags: server info (machine/OPC server), dynamic tags (PV/MV), static tags
      (DR/SH/SL/MSH/MSL) with a tag-vs-static-value toggle per field, optional tags
      (SV/Mode/ModeAttribute/P/I/D tuning-constant tags).
- [ ] REQUIRED — real server-side/range validation on every numeric field, not just client-side
      keystroke filtering (see §2 for Relay Amplitude specifically).
- [ ] REQUIRED — Settings: DCS/PLC template selection, add/delete templates, template field
      editing (see §7).
- [ ] NOT PLANNED — any licensing/loop-status UI. BHTune has no license gating of any kind.
- [ ] REQUIRED — Help — offline/online help content.
- [x] REQUIRED — a live PV/MV trend chart in the web GUI (`frontend-live-stream` phase,
      `uPlot` per `AGENTS.md`), handling high-rate streaming updates (multiple times per second).

## 4. MRFT lifecycle

- [ ] REQUIRED — start-of-test validation: required fields, conditional tag-or-static-value
      requirements per toggle state, tag-changed-without-refill guard.
- [ ] REQUIRED — template-to-engine field assignment: copying the active DCS/PLC template's
      settings onto the tuning engine before a test starts.
- [ ] REQUIRED — static-value reads at test start (DR/SH/SL/MSH/MSL, mode, mode attribute) when
      not using OPC tags for them.
  - [ ] REQUIRED — controller-direction resolution as a proper `Direct`/`Reverse` enum, not a
        string-comparison sentinel.
- [ ] REQUIRED — initial OPC reads (PV, mode, mode attribute, controller direction, PV/MV ranges,
      MV).
- [ ] REQUIRED — MV boundary clamp for both the upper bound and the lower bound, using a
      dimensionally consistent formula on both sides (see `AGENTS.md`'s correctness-critical
      design details, item 1).
- [ ] REQUIRED — controller-mode-to-Manual transition at test start, including the mode-attribute
      "Program" write, the Manual-mode write with a pre-write settle delay, and capturing the
      setpoint value when transitioning from Auto.
- [ ] REQUIRED — a fixed-interval polling cadence for MRFT evaluation.
- [ ] REQUIRED — the OPC PV read happening every tick even during pre/post-test delay padding,
      with switch evaluation itself gated separately.
- [ ] REQUIRED — switch-needed decision: setpoint/PV difference tracking, per-cycle peak/trough
      tracking, hysteresis via the beta constant, next-step MV sign and magnitude, a deadband
      before a switch is considered "needed" (not switching on negligible MV deltas), and
      noise-protection delay gating (except for the very first switch).
- [ ] REQUIRED — switch-performed action: switch counter increments, skip-switch handling before
      peak/trough recording starts, cycles-completed/remaining recomputation, and the
      final-step snap-back to the initial MV value instead of a full relay step.
  - [ ] REQUIRED — record the tick's already-captured timestamp for the switch, never a fresh
        wall-clock read (see `AGENTS.md`, item 3).
- [ ] REQUIRED — completion condition (total switches reached) and the resulting tuning-math +
      PID-parameter calculation pass.
- [ ] REQUIRED — MRFT period calculation using full elapsed-time precision, not truncated
      hours/minutes/seconds (see `AGENTS.md`, item 2).
- [ ] REQUIRED — frequency, peak/trough summation (order depends on initial MV sign and action
      direction), PV amplitude (raw and percent), and the per-response-level Kp/Ti/Td tuning-
      constant formulas.
- [ ] REQUIRED — unit conversion: Kp vs. Proportional Band; Ti/Td in seconds or minutes per
      template setting; Ti→Ri or Ti→Ki; Td→Kd — per the active template's type/unit settings.
- [ ] REQUIRED — write-back semantics: never automatic, requires explicit per-response-level
      confirmation; P is always written; I is written as a neutralizing sentinel value (not simply
      skipped) when the controller type is P-only; D is written only for full PID.
- [ ] REQUIRED — abort/error restoration: write the original MV back before anything else, then
      restore mode/mode-attribute/setpoint to their captured initial values (only if they were
      changed at test start).

## 5. Step Test lifecycle — DEFERRED

Everything in this section is deferred past v1, not permanently out of scope.

- [ ] DEFERRED — manual/observational Step Test: subscription-driven PV monitoring (not
      polling), recording PV/SV/MV/P/I/D on each PV change.
  - Blocked on adding a subscription/streaming RPC to `opcda-bridge` (see `AGENTS.md`) — this is
    fundamentally subscription-driven, unlike MRFT's unary polling reads.
  - [ ] REQUIRED, once implemented — generate any tabular/CSV export's header and data row from a
        single ordered field-name list, never two independently hand-written strings (see
        `AGENTS.md`, item 5).

## 6. Logging → SQLite

- [ ] NOT PLANNED — any log encryption. Nothing in BHTune is encrypted; plain, open SQLite is a
      deliberate choice, not a gap.
- [ ] REQUIRED — every tuning-relevant static config value and every per-tick dynamic state value
      recorded as SQLite rows (`tune_runs`/`tune_samples`/`tune_results` — `db-schema` phase).
- [ ] NOT PLANNED — writing any log/export file outside the app's own data directory or a path the
      user explicitly chooses. No implicit "wherever the process happened to start" locations.

## 7. Settings / DCS-PLC templates

- [ ] REQUIRED — the four shipped templates (Yokogawa CentumVP, Honeywell Experion, Schneider
      Modicon, Allen-Bradley PlantPAx) with real field values (`db-seed-templates` phase).
- [ ] REQUIRED — the template schema: revert-mode flag; proportional/integral/derivative
      type-and-unit settings (as proper enums, never magic display strings); manual/auto mode
      values; mode-attribute "Program" value; controller-action-direct value.
- [x] REQUIRED — add/delete custom templates.
- [x] REQUIRED — persist templates to SQLite as the source of truth, seeded from an embedded,
      community-contributable TOML catalog (`template-catalog`, done — see `AGENTS.md`'s
      "Community DCS/PLC template catalog"); JSON import/export, TOML export
      (`template export --format toml`), and `template delete` are all done (`template-cli` —
      see AGENTS.md's "Multi-template import, TOML export, and `template delete`"), and
      auto-loading a user-supplied catalog file is done (`template-user-catalog` — see
      AGENTS.md's "Auto-loading a user template catalog").
- [ ] REQUIRED — assigning the entire active template onto the tuning engine immediately before a
      test starts (§4).

## 8. Licensing — NOT PLANNED

- [ ] NOT PLANNED — any usage-gating, dongle/license-check, or "licensed loop count" concept, in
      any form. BHTune is free, open-source software; there is no enforcement mechanism to build.

## 9. State / data model

- [ ] REQUIRED — connection/identity fields (host machine, OPC server ID, run identifier).
- [ ] REQUIRED — the full OPC tag-name set (PV, MV, SV, mode, mode attribute, SH/SL, MSH/MSL, DR,
      P/I/D tuning-constant tags).
- [ ] REQUIRED — tag-vs-static-value toggle flags for SH/SL/MSH/MSL/DR/Mode/ModeAttribute/SV,
      modeled so the toggle and the value can never disagree (e.g. a single `TagOrValue<T>` sum
      type rather than a separate bool flag plus two independently-settable fields).
- [ ] REQUIRED — test parameters (relay amplitude, controller type, process type, cycles to
      skip/count, noise protection delay, MRFT delay time).
- [ ] REQUIRED — values captured at test start (initial PV/MV/SV/mode/mode-attribute, PV/MV
      ranges, controller direction, action multiplier).
- [ ] REQUIRED — the tuning-constant lookup tables (beta and C1/C2/C3 per process type × response
      level × controller type), sized to exactly the 6 real process types with no unreachable
      extra rows (see `AGENTS.md`, item 4).
- [ ] REQUIRED — runtime MRFT state (MV current/next-step/signs, switch-required/enabled flags,
      valve-switch value, setpoint/PV difference, hysteresis, per-cycle peak/trough tracking,
      switch/peak/trough counters, cycles completed/remaining, switch-count targets, current and
      historical switch timestamps, peak/trough/switch-time series).
- [ ] REQUIRED — results (period, frequency, peak/trough sums, PV amplitude raw and percent,
      per-response-level Kp, Ti, Td, and the final unit-converted P/I/D values per response
      level).
- [ ] REQUIRED — state flags (test in progress, completed successfully, delay-recording complete,
      OPC error).
- [ ] REQUIRED — significant-digit-aware display formatting for live PV/MV readouts. Decide up
      front whether exact significant-digit string formatting is needed, or whether
      straightforward numeric rounding is an acceptable, documented simplification for
      display-only purposes (not for anything replay-validated) — the two are not interchangeable
      (see `AGENTS.md`, item 15).

## 10. Cross-cutting requirements

- [ ] REQUIRED — `f32` precision throughout the tuning engine, matching typical OPC DA analog-tag
      width end-to-end and keeping golden-master replay comparisons exact.
- [ ] REQUIRED — one unified async concurrency model in `bhtune-core` for both MRFT (polling) and
      Step Test (subscription, once implemented) — not two different mechanisms for conceptually
      the same job.
- [x] REQUIRED — `cli-safety` guardrails for unattended/scheduled tuning: mandatory wall-clock
      timeout with automatic abort-and-restore, explicit opt-in required for any run that writes
      PID constants without a human present. A follow-up live-plant safety hardening pass
      (Phase 6.5) closed nine further findings covering Ctrl+C/timeout cancellation reaching an
      in-flight backend call, guaranteed restoration on every exit path, input validation, OPC
      quality enforcement, PID write-back pre-read/verify/audit/rollback, a WAL-safe database
      restore, an `--output json` stdout contract, and a per-run template/tag snapshot — see
      AGENTS.md's "Live-plant safety hardening" section.
