---
sidebar_position: 4
---

# Templates and configuration

Templates define the tag suffixes, raw mode values, and PID unit conventions used to turn a
single process-variable tag into a complete loop mapping. Configuration controls installation-
wide quality and retention behavior plus MRFT timing and safety values.

## Templates

The template list shows built-in, catalog, and user-owned templates. Built-in and catalog rows
are re-seeded from their source on server startup; only user-owned templates can be edited in
place.

{/* web-ui-screenshot: full-template-list */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-template-list.png?v=52c1bb74fc28">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-template-list.png?v=52c1bb74fc28" alt="BHTune Templates page listing available DCS and PLC templates" />
  </a>
  <figcaption>The template list identifies the available mapping catalog and its ownership origin.</figcaption>
</figure>

{/* web-ui-screenshot: full-template-detail */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-template-detail.png?v=af48ae6715dd">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-template-detail.png?v=af48ae6715dd" alt="BHTune template detail page showing identity, behavior, tag suffixes, and mode values" />
  </a>
  <figcaption>Template detail groups the mapping into identity, behavior, suffixes, and raw mode values.</figcaption>
</figure>

The create and edit forms use the same grouped fields as the read-only detail view. A user's
template name is immutable during editing; rename by deleting and recreating it instead.

{/* web-ui-screenshot: full-template-create */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-template-create.png?v=34747585e504">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-template-create.png?v=34747585e504" alt="BHTune New template form with identity, behavior, suffix, and mode-value sections" />
  </a>
  <figcaption>Create a user-owned template by filling the same fields documented in the template catalog reference.</figcaption>
</figure>

{/* web-ui-screenshot: full-template-edit */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-template-edit.png?v=94ad5ceae468">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-template-edit.png?v=94ad5ceae468" alt="BHTune edit-template form with the existing template name locked" />
  </a>
  <figcaption>Edit preserves the template identity; the locked Name field prevents an unsupported rename.</figcaption>
</figure>

For the complete field reference, catalog format, validation rules, and the
`Area01.FIC101.OUT` to `Area01.FIC101.PV` suffix example, see
[DCS/PLC templates](../../dcs-templates.md).

## Configuration

The Configuration page edits the same `bhtune.toml` used by the CLI and server. It exposes
global `Uncertain`-quality policy and history retention, along with tuning timing and safety
settings. Startup-only values such as the database path, bind address, log directory, and
template-catalog path remain file/configuration settings.

{/* web-ui-screenshot: full-config */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-config.png?v=7ee5d37a7587">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-config.png?v=7ee5d37a7587" alt="BHTune Configuration page showing OPC quality, retention, timing, safety, and guidance sections" />
  </a>
  <figcaption>Configuration keeps global quality, retention, timing, and safety policy in one reviewed surface.</figcaption>
</figure>

Good readings always pass. Uncertain readings follow the global policy, which defaults to
allowed; Bad readings are always rejected. The policy is captured with each run and write/
revert operation, so a later configuration change cannot reinterpret existing history.

Retention is age-based and off by default. Saving a new retention value changes future sweeps;
it does not immediately delete history. The server periodically applies the active policy, and
the CLI exposes the same policy through `bhtune history prune`.
