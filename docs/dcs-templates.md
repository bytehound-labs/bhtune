# DCS/PLC templates

A **template** tells BHTune how one DCS/PLC vendor's PID controller block expresses itself
over OPC DA: what the proportional/integral/derivative terms are called and what units
they're in, what suffix each related tag uses, and what raw values a mode tag holds for
Manual/Auto. Selecting a template is what lets `bhtune tune` turn a single process-variable
tag into a complete, correct tag set for a given control system.

BHTune ships four templates out of the box (Yokogawa CentumVP, Honeywell Experion,
Schneider Modicon, Allen-Bradley PlantPAx), and templates are an ordinary, contributable
data file — adding support for another DCS/PLC family is a pull request against a TOML
file, not a Rust change. **Contributions of new templates are very welcome.** The goal is
a community-maintained library that eventually covers as many control systems as possible;
see [Contributing a template](#contributing-a-template) below.

## Where templates come from

A template row in BHTune's database has one of three origins:

| Origin    | Source                                                                                                                 | Re-seeded on every startup? |
| --------- | ---------------------------------------------------------------------------------------------------------------------- | --------------------------- |
| `builtin` | The catalog embedded in the `bhtune` binary itself (`crates/bhtune-core/templates/builtin.toml`)                       | Yes                         |
| `catalog` | Your own catalog file, auto-loaded on startup (`--templates`/`BHTUNE_TEMPLATES`, see the README's Configuration table) | Yes                         |
| `user`    | Hand-imported (`bhtune template import`) or otherwise created                                                          | No                          |

`builtin` and `catalog` rows are re-applied from their source file every time `bhtune`
starts, so editing the file is how you change them; `user` rows are left alone. This is why
there's no way to permanently delete a `builtin`/`catalog` template with `bhtune template
delete` alone — it comes back on the next start unless you also remove it from the file
that seeds it.

## The TOML catalog format

Both the embedded catalog and your own catalog file use the same shape: a TOML array of
tables, one `[[template]]` block per template.

```toml
[[template]]
name = "Yokogawa CentumVP"
revert_mode = true
proportional_type = "band"
integral_type = "reset_time"
integral_unit = "seconds"
derivative_type = "derivative_time"
derivative_unit = "seconds"
process_variable_suffix = "PV"
manipulated_variable_suffix = "MV"
setpoint_variable_suffix = "SV"
controller_direction_suffix = "DR"
controller_mode_suffix = "MODE"
mode_attribute_suffix = ""
upper_pv_range_suffix = "SH"
lower_pv_range_suffix = "SL"
upper_mv_range_suffix = "MSH"
lower_mv_range_suffix = "MSL"
proportional_constant_suffix = "P"
integral_constant_suffix = "I"
derivative_constant_suffix = "D"
mode_manual_value = "MAN"
mode_auto_value = "AUT"
controller_action_direct_value = "0"
versions = ["R5", "R6"]
description = "Yokogawa CENTUM VP DCS: PID station function block, proportional-band convention, MAN/AUT mode tag."
source = "Field-confirmed tag mapping from a live CENTUM VP deployment."
```

This is the actual built-in Yokogawa CentumVP template, used as the running example for
every field below.

## Field reference

### Identity

| Field         | Type                       | Meaning                                                                                                                                                                                                                                                                                                                          |
| ------------- | -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `name`        | string, required           | The unique key. `bhtune template show/export/delete` all look a template up by this. Two templates covering different releases of the same vendor get **different names** (e.g. `"Yokogawa CentumVP"` vs. a future `"Yokogawa CentumVP R7"`), not the same name edited in place — see [Versions](#versions-not-a-verified-flag). |
| `versions`    | array of strings, optional | The DCS/PLC releases this mapping is known to apply to, in that vendor's _own_ version-naming convention (`"R6"`, `"Unity Pro V8.1"`, `"4.0"` — whatever an engineer at that system would actually recognize). May be empty if you don't know. Not an exhaustively tested compatibility matrix — see below.                      |
| `description` | string, optional           | A short free-text summary of the control system and PID convention this template targets.                                                                                                                                                                                                                                        |
| `source`      | string, optional           | A citation for where the mapping came from — a manual, a reference document, a field deployment. Provenance, not a correctness guarantee (see below).                                                                                                                                                                            |

### PID convention

| Field               | Type    | Values                                                                                          | Meaning                                                                                                                                                                                     |
| ------------------- | ------- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `proportional_type` | enum    | `"gain"` (Kp, dimensionless) or `"band"` (PB%, `PB = 100 / Kp`)                                 | How this DCS expresses the proportional term.                                                                                                                                               |
| `integral_type`     | enum    | `"reset_time"` (Ti), `"reset_rate"` (Ri, `Ri = 1 / Ti`), or `"reset_gain"` (Ki, `Ki = Kp / Ti`) | How this DCS expresses the integral term.                                                                                                                                                   |
| `integral_unit`     | enum    | `"seconds"` or `"minutes"`                                                                      | The time unit `integral_type` is expressed in, when it's time-based.                                                                                                                        |
| `derivative_type`   | enum    | `"derivative_time"` (Td) or `"derivative_gain"` (Kd, `Kd = Kp * Td`)                            | How this DCS expresses the derivative term.                                                                                                                                                 |
| `derivative_unit`   | enum    | `"seconds"` or `"minutes"`                                                                      | The time unit `derivative_type` is expressed in.                                                                                                                                            |
| `revert_mode`       | boolean | —                                                                                               | Whether the loop's controller mode is switched back to its original mode (e.g. Auto/Cascade) after a completed test. Has no effect if the loop was already in Manual when the test started. |

### Tag suffixes

BHTune derives a loop's full tag set from a single process-variable (PV) tag plus these
suffixes: it replaces everything after the last `.` or `!` in the PV tag's path with each
suffix in turn. For a PV tag of `Unit1.LIC101.PV`, the Yokogawa template above derives the
manipulated-variable tag as `Unit1.LIC101.MV`, the setpoint tag as `Unit1.LIC101.SV`, and so
on for every non-empty suffix field.

| Field                                                                                      | Meaning                                                                                                                                                      |
| ------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `process_variable_suffix`                                                                  | The PV tag's own suffix — required.                                                                                                                          |
| `manipulated_variable_suffix`                                                              | The controller output/MV tag — required.                                                                                                                     |
| `setpoint_variable_suffix`                                                                 | The setpoint tag.                                                                                                                                            |
| `controller_direction_suffix`                                                              | The tag holding Direct/Reverse controller action.                                                                                                            |
| `controller_mode_suffix`                                                                   | The tag holding the controller's Manual/Auto mode.                                                                                                           |
| `mode_attribute_suffix`                                                                    | A _second_ mode-related tag some DCS families require to permit external OPC writes (e.g. Honeywell Experion's `MODEATTR`, which must read "Program" first). |
| `upper_pv_range_suffix` / `lower_pv_range_suffix`                                          | The PV's engineering-unit range.                                                                                                                             |
| `upper_mv_range_suffix` / `lower_mv_range_suffix`                                          | The MV's engineering-unit range.                                                                                                                             |
| `proportional_constant_suffix` / `integral_constant_suffix` / `derivative_constant_suffix` | The tags the calculated P/I/D constants are written back to.                                                                                                 |

An **empty string** (`""`) means "this DCS has no such concept" — for example, Yokogawa
CentumVP has no separate mode-attribute tag, so `mode_attribute_suffix = ""`. This is
different from leaving a field out entirely, which isn't valid for suffix fields (they're
all required strings, just possibly empty).

### Raw tag values

| Field                                   | Meaning                                                                                                                                                                                                                                                                  |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `mode_manual_value` / `mode_auto_value` | The literal raw value `controller_mode_suffix`'s tag holds for Manual and Auto, respectively (e.g. Yokogawa uses the literal text `"MAN"`/`"AUT"`; Schneider Modicon uses the literal text `"false"`/`"true"`). Required whenever `controller_mode_suffix` is non-empty. |
| `mode_attribute_program_value`          | The raw value `mode_attribute_suffix`'s tag must hold for OPC writes to be accepted (e.g. Honeywell Experion's `"2"` for Program). Required whenever `mode_attribute_suffix` is non-empty; otherwise omit it.                                                            |
| `controller_action_direct_value`        | The raw value `controller_direction_suffix`'s tag holds when the controller is Direct-acting (every built-in template uses the literal text `"0"`, but this is vendor-specific, not assumed).                                                                            |

All of these are **strings**, even when they look like booleans or numbers (`"false"`,
`"true"`, `"0"`). This matters: unquoted `false`/`true`/`0` in TOML are boolean/integer
values, not the strings BHTune's schema expects, and would fail validation or silently
mean something different. Always quote them.

## Validation

Every template — built-in, from your catalog file, or hand-imported — is validated before
it can be used:

- `name` must be non-empty (after trimming whitespace).
- `process_variable_suffix` and `manipulated_variable_suffix` must both be non-empty —
  without them, no tag set can be derived at all.
- If `controller_mode_suffix` is set, both `mode_manual_value` and `mode_auto_value` must
  be non-empty.
- If `mode_attribute_suffix` is set, `mode_attribute_program_value` must be present.

A template that fails validation is rejected outright — at catalog-parse time for the
embedded/user catalog, and at `bhtune template import` time for a single-template JSON
file — rather than being accepted and only failing later, mid-tune, against a live loop.

## Versions, not a "verified" flag

`versions` records the releases a mapping is _known_ to apply to, framed as "current when
this mapping was authored," not as an exhaustively tested compatibility matrix. When a
newer release of a DCS/PLC changes its tag conventions, add a **new** `[[template]]` entry
with its own `name` and `versions` list — never edit an existing entry's suffixes in place.
Sites still running the older release depend on the mapping exactly as it's already
written; changing it out from under them is a silent regression for someone else.

There is deliberately no separate "verified" trust field. Everything accepted into the
catalog is treated as verified, and genuine mapping errors are fixed as bugs when they
surface — a three-state trust badge would need someone to adjudicate and maintain it per
template, and a stale or wrong badge is worse than no badge at all.

## Contributing a template

1. Start from an existing `[[template]]` block in
   [`crates/bhtune-core/templates/builtin.toml`](../crates/bhtune-core/templates/builtin.toml)
   closest to your control system, or export a template you've already built via the CLI
   (see below) as a starting point.
2. Fill in every suffix field for your DCS/PLC's PID block, its raw mode values, and a
   `versions` entry naming the release(s) you tested against.
3. Add a short `description` and a `source` citing where the mapping came from (a manual,
   a function block reference, a field deployment).
4. Open a pull request against `builtin.toml`. CI parses and validates the whole file on
   every push, so a malformed or incomplete contribution fails the build rather than
   merging silently broken.
5. If you'd rather keep a template private to your own site instead of (or before)
   contributing it upstream, put it in your own catalog file instead — see the README's
   Configuration table for the `--templates`/`BHTUNE_TEMPLATES` auto-loaded catalog path.

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for the repository's general contribution
workflow (branching, commit style, the CLA).

## CLI usage

```sh
# List every template (built-in, catalog, and user-imported)
bhtune template list

# Show one template's full field detail as JSON
bhtune template show "Yokogawa CentumVP"

# Export a template as a starting point -- JSON for a single-template copy,
# or TOML for a catalog-ready, PR-able [[template]] block
bhtune template export "Yokogawa CentumVP" ./my-template.json
bhtune template export "Yokogawa CentumVP" ./my-template.toml --format toml

# Import a template -- a single JSON template, or a multi-template TOML catalog
# (auto-detected from the file's content, not its extension)
bhtune template import ./my-template.json
bhtune template import ./site-catalog.toml

# Delete a template (refuses if a saved loop still references it)
bhtune template delete "My Custom Template"
```
