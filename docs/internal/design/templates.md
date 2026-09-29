# DCS/PLC templates

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Community DCS/PLC template catalog (`template-catalog`)

Turns the four built-in DCS/PLC templates from hardcoded Rust constructor functions into a
contributable data file, so adding a new control-system family becomes a TOML pull request
rather than a Rust change — motivated by the goal of eventually shipping a full library of
DCS/PLC systems, which doesn't scale if every contributor has to learn the workspace layout
and get a Rust PR reviewed.

- **Format: TOML, not JSON or YAML.** The legacy app's `SettingsTemplates.json` had the right
  _content_ but the wrong _shape_ for a community catalog. JSON has no comments, and a shared
  catalog needs inline provenance (which manual a suffix came from, why a field is blank) —
  that alone rules it out as an authoring format. YAML's implicit typing is an active footgun
  here: `mode_manual_value`/`mode_auto_value`/`controller_action_direct_value` are, for some
  templates, literally the _strings_ `"true"`/`"false"`/`"0"` — YAML would silently coerce
  unquoted forms of these to bool/int on exactly the fields that decide whether a loop gets
  put into Manual. `toml` was already a dependency (`bhtune-cli`'s single-template import/
  export); the mainstream YAML crates are unusable under this project's `cargo deny` gate
  (`serde_yaml` is deprecated/archived, its `serde_yml` fork carries RUSTSEC-2025-0068). JSON
  import/export stays supported for interop; a `toml` export format is done (`template-cli`,
  see below) so the contribution loop is export → annotate → PR.
- **Embedded, not read from disk.** `crates/bhtune-core/templates/builtin.toml` is
  `include_str!`-compiled into the binary, so a shipped binary can never be broken by a
  missing or hand-edited data file, while contributors still edit a plain text file, not
  Rust. `built_in_templates()` is now `parse_catalog(BUILTIN_CATALOG).expect(...)`; a unit
  test parses and validates the embedded file, so a malformed contribution fails CI rather
  than shipping.
- **`parse_catalog(&str) -> Result<Vec<DcsTemplate>, TemplateError>`** is the pure parsing
  entry point — deserializes a private `Catalog { #[serde(rename = "template")] templates:
Vec<DcsTemplate> }` wrapper (TOML's array-of-tables idiom, one `[[template]]` block per
  entry) and then calls `.validate()` on every template, so a syntactically valid but
  semantically incomplete contribution (e.g. a mode suffix with no manual/auto value) is
  rejected at parse time, not mid-tune. Parsing a `&'static str` embedded at compile time is
  not I/O, so `bhtune-core`'s "no I/O, no clock, no async" purity rule is preserved — all
  _file_ reading stays in `bhtune-cli`, which reuses this exact function to load a user
  catalog from disk (`template-user-catalog`, done — see below).
- **`DcsTemplate::validate()`** mirrors the `LoopConfig::validate` precedent from
  `cli-safety`: non-empty `name` (trimmed); non-empty `process_variable_suffix`; non-empty
  `manipulated_variable_suffix`; if `controller_mode_suffix` is set, both
  `mode_manual_value` and `mode_auto_value` must be non-empty; if `mode_attribute_suffix` is
  set, `mode_attribute_program_value` must be `Some`. Runs on every catalog template (built-
  in, contributed, or user-catalog — `template-user-catalog`'s `load_user_templates` reuses
  `parse_catalog` directly, so the same validation applies with no separate code path) and,
  since `template-cli`, on `template import`'s single-JSON-template path too (`import_one`
  calls it explicitly, since that path parses with plain `serde_json::from_str` rather than
  going through `parse_catalog`) — a garbage template can no longer be imported and only
  fail much later, mid-tune.
- **`TemplateError`** is a hand-rolled `Display`/`std::error::Error` enum (no `thiserror`,
  matching `bhtune-core`'s existing `RangeError`/`LoopConfigError` convention): `Toml(toml::
de::Error)` (`source()` delegates to the wrapped error), `EmptyName`, `EmptyField { name,
field }`, `MissingModeValue { name, field }`, `MissingModeAttributeProgramValue { name }`.
  `toml::de::Error` (the resolved `toml 1.1.4+spec-1.1.0`) already derives `Clone`/`PartialEq`
  and implements `Display`/`Error`, so it's stored directly rather than flattened into a
  `String` — no information is lost converting a parse error into this crate's own error type.
- **New `DcsTemplate` fields, all `#[serde(default)]`** so both the TOML catalog and any
  existing JSON import/export stay backward-compatible: `versions: Vec<String>` (the DCS/PLC
  releases a template's tag conventions are known to apply to — see "Per-version templates"
  below), `description: Option<String>`, `source: Option<String>` (a documentation citation
  for where the tag mapping came from). Deliberately **no "verified" trust field** — an
  earlier draft proposed a hardware/documentation/unverified enum, dropped because it would
  need someone to adjudicate and maintain it per template, and a stale "verified" badge is
  worse than none; everything accepted into the catalog is treated as verified, and real
  errors get fixed as bugs when they surface.
- **Per-version templates, not per-vendor.** DCS vendors change tag conventions across major
  releases, so a single "Yokogawa CentumVP" entry can silently be wrong for a newer release.
  Each template carries a `versions` list of the releases it's known to apply to (e.g.
  `["R5", "R6"]`); when a release changes conventions, the contribution pattern is a **new
  template entry** with its own name and `versions` list, never editing an existing one in
  place, since sites on the older release still depend on that exact mapping. `name` stays
  the single unique key, so no lookup code has to change. The four seeded templates'
  `versions` reflect when each mapping was actually authored (~2015–2016), not an exhaustive
  tested matrix — recorded with a "current as of authoring" comment in `builtin.toml` so a
  later reader doesn't over-read the list as a coverage guarantee: Yokogawa CentumVP `["R5",
"R6"]` (field-confirmed), Honeywell Experion `["R400", "R410", "R430"]`, Schneider Modicon
  `["Unity Pro V8.0", "Unity Pro V8.1", "Unity Pro V11.0"]`, Allen-Bradley PlantPAx `["3.0",
"3.5", "4.0"]`.
- **`toml` promoted to `[workspace.dependencies]`** now that both `bhtune-cli` (single-
  template JSON/TOML import/export) and `bhtune-core` (the embedded catalog) consume it, per
  the root `Cargo.toml`'s own documented convention of promoting on a second consumer.
- **`bhtune-db` fallout, resolved by `template-provenance`.** `DcsTemplate` is `bhtune-db`'s
  own row type for `dcs_templates` (no separate DTO — see `db-schema`'s design note), so
  `row_to_dcs_template` had to start constructing the three new fields the moment
  `template-catalog` added them to `DcsTemplate` itself. They were read back as empty/`None`
  placeholders for one commit, documented in place as a stopgap; `template-provenance` (below)
  closed the gap immediately after by adding real `versions_json`/`description`/`source`
  columns, so every field now round-trips through actual storage rather than a placeholder.

**Testing approach.** 24 tests in `bhtune-core/src/template.rs` (18 new): the embedded
catalog parses and every built-in validates; each built-in's `versions`/`description`/
`source` match the researched seed values above; a minimal valid TOML template parses;
malformed TOML is rejected as `TemplateError::Toml`; every `validate()` branch is exercised
individually via targeted `str::replace` edits on a shared minimal-valid-TOML fixture (empty
PV suffix, empty MV suffix, mode suffix missing manual value, mode suffix missing auto value,
mode-attribute suffix missing program value); `TemplateError`'s `Display`/`std::error::Error`/
`source()` behavior is covered for every variant, not just the ones a validation branch
happens to construct. `cargo llvm-cov` confirms 100% line coverage of the new code.

### `dcs_templates.origin` replaces `is_builtin` (`template-provenance`)

`template-catalog` (above) taught `DcsTemplate` that a template can come from more than one
place, but `dcs_templates` itself still only had a two-state `is_builtin BOOLEAN` — no room
for a third state, which `template-user-catalog` (see "Auto-loading a user template catalog"
below) needs: a row seeded from a site's own `templates.toml` is neither a shipped built-in
nor a hand-imported user template, and treating it as either would break one of the two.
Done now, pre-release, specifically to avoid a later `ALTER TABLE` migration once real
installs exist.

- **`origin TEXT CHECK (origin IN ('builtin', 'catalog', 'user'))`** replaces `is_builtin
INTEGER`. `bhtune_db::models::TemplateOrigin` (`Builtin`/`Catalog`/`User`) — previously
  defined only for `tune_runs.template_origin` (`safety-run-snapshot`, with a temporary
  `from_is_builtin` bridge method) — moved to be `dcs_templates`' own type, since that's now
  its primary use; `tune_runs.template_origin` reuses the same enum for its run-start
  snapshot, and `from_is_builtin` was deleted as dead code now that `dcs_templates.origin` is
  real. `builtin` and `catalog` rows are re-upserted from their respective files on every
  startup; `user` rows (hand-imported or, eventually, GUI-created) are never auto-touched.
- **New `versions_json`/`description`/`source` columns** replace the placeholder empty/`None`
  values `template-catalog` had to read back (see the "resolved by `template-provenance`"
  bullet above) with the template's real, already-authored data — `versions_json` follows the
  same `CHECK (json_valid(...))`-plus-`serde_json`-round-trip idiom as `tags_json`/
  `template_snapshot_json`, needing no new `DbError` variant (`InvalidJsonShape`'s doc comment
  was broadened to mention it).
- **`seed_builtin_templates` generalized into `seed_templates(pool, templates, origin, now)`**,
  with `seed_builtin_templates` kept as a thin wrapper (`seed_templates(pool,
built_in_templates(), TemplateOrigin::Builtin, now)`) rather than renamed, since `bhtune-cli`
  already has established callers of the original name. `template-user-catalog` (see below)
  is the first real caller of `seed_templates` directly, with `TemplateOrigin::Catalog`. The
  `SkippedUserOwned` outcome generalizes the same way the boolean did: a row exists but its
  `origin` differs from the one being seeded, so it belongs to a different catalog/seed pass
  and is left untouched.
- **Three new tests in `tests/schema.rs`** cover what the schema alone had never proven: that
  the `origin` `CHECK` constraint actually rejects an invalid value (via a raw `UPDATE`, since
  `dcs_templates` has too many non-defaulted `NOT NULL` columns to hand-write a full raw
  `INSERT` the way the simpler tables' precedent tests do), that all three `origin`
  variants — including `Catalog`, which no production code path produces yet — round-trip
  through `DcsTemplateRow::get`, and that `row_to_dcs_template`'s `versions_json` decode-error
  path actually fires for JSON that is syntactically valid (satisfying the `CHECK`) but the
  wrong shape (`"123"`, a bare JSON number, isn't a `Vec<String>`) — the `CHECK` constraint
  alone only proves bad _syntax_ is rejected at the SQL layer, not that the Rust-level shape
  mismatch is handled once past it.

### Auto-loading a user template catalog (`template-user-catalog`)

`template-catalog`/`template-provenance` (above) made the catalog format and the database's
three-way `origin` real, but nothing yet populated `TemplateOrigin::Catalog` with real data —
a site's own `templates.toml` was still not read by anything. `bhtune-cli` now auto-loads one
on every startup, mirroring `bhtune.toml`'s own `cli-config` precedence chain exactly rather
than inventing a new pattern.

- **Resolution order: `--templates` > `BHTUNE_TEMPLATES` > `templates` config key > platform
  default.** `config::load_user_templates(cli_templates, config, xdg_config_home, home,
appdata, is_windows)` in `crates/bhtune-cli/src/config.rs` mirrors `load_config`'s own
  split between an _explicit_ path (CLI flag, already folded in by clap's `env =
"BHTUNE_TEMPLATES"`, or the config-file key) and the _auto-discovered default_ path — but
  it is a 4-tier chain, one tier deeper than `load_config`'s own 2-tier bootstrapping case,
  because `bhtune.toml`'s own path obviously can't be configured from inside itself, whereas
  the templates path _can_ have a config-file-key tier since `bhtune.toml` is already loaded
  by the time templates are resolved.
- **`templates_path_from(...)` mirrors `config_path_from` (the config directory), not
  `default_db_path_from`/`default_log_dir_from` (the data directory).** `templates.toml`
  lives in the same directory as `bhtune.toml` — `$XDG_CONFIG_HOME/bhtune/templates.toml` on
  Linux/macOS (falling back to `$HOME/.config/bhtune/` if unset), `%APPDATA%\bhtune\
templates.toml` on Windows — since both are per-user hand-edited settings files, not
  persistent application data.
- **Missing-file semantics depend on how the path was resolved, matching `bhtune.toml`'s own
  rule.** An auto-discovered default path that doesn't exist is `Ok(None)` — not an error, and
  the common case, since most installs never create `templates.toml` at all. An _explicit_
  path — from `--templates`/`BHTUNE_TEMPLATES` or the config file's `templates` key — that
  doesn't exist is a hard error naming the path, exactly like an explicit `--config` path that
  doesn't exist. A file that exists but fails to parse (malformed TOML) or fails
  `DcsTemplate::validate()` (e.g. a mode suffix with no manual/auto value) is always a hard
  error regardless of how the path was resolved, naming the file and the problem.
- **Reuses `bhtune_core::template::parse_catalog` directly** — the same function
  `template-catalog` built for the embedded built-in catalog already parses the `[[template]]`
  TOML shape _and_ calls `.validate()` on every template, so a single call handles both
  "shape" and "content" validation with no new parsing code in `bhtune-cli` at all.
- **`db::open`'s signature grew a `user_templates: Option<Vec<DcsTemplate>>` parameter.** It
  seeds the built-ins first (as before), then — only if `Some` — seeds the user catalog via
  `bhtune_db::seed_templates(&pool, templates, TemplateOrigin::Catalog, now)`, the first real
  production caller of the `Catalog` origin (previously exercised only by one round-trip
  test in `template-provenance`). `lib.rs`'s `run_with_cli_and_ctrl_c` calls
  `config::load_user_templates(...)` right after resolving the database path and before
  `db::open`, propagating a load/parse/validation failure through the same `fail()` exit path
  every other startup error uses.
- **`--templates <PATH>` global CLI flag**, placed alongside `--db`/`--config` in `args.rs`,
  with `env = "BHTUNE_TEMPLATES"` matching the other global flags' `BHTUNE_*` convention.

**Testing approach.** 2 new tests in `db.rs` (seeding a user catalog tags rows with
`TemplateOrigin::Catalog`; reseeding the same catalog is idempotent), 2 extended tests in
`args.rs` (the new flag/env var/default), and 16 new tests in `config.rs`: 5 for
`templates_path_from` (Windows with/without `%APPDATA%`, Unix via `$XDG_CONFIG_HOME`, Unix
via `$HOME` fallback, Unix with neither set) and 11 for `load_user_templates` (nothing
resolves → `None`; an auto-discovered path that's missing → `Ok(None)`, not an error; an
explicit CLI-flag path that's missing → error; an explicit config-key path that's missing →
error; a valid file parses; malformed TOML → error; a template failing `validate()` → error;
the generic-I/O-error branch, e.g. reading a directory as if it were a file → error; the CLI
flag winning over the config key; the config key being used when there's no CLI flag). Plus
one `lib.rs` integration test proving `run_with_cli` itself (not just the lower-level
`config::load_user_templates` unit) surfaces a broken `--templates` path as exit failure
before ever calling `db::open`. `cargo llvm-cov` confirms 100% line coverage of every line
this todo added or touched.

### Multi-template import, TOML export, and `template delete` (`template-cli`)

`template-catalog`/`template-provenance`/`template-user-catalog` (above) made TOML catalogs a
real, auto-loaded data source, but `bhtune template import`/`export` still only understood a
single JSON template, and there was no way at all to remove a template once imported or
auto-seeded — a real dead end now that startup auto-loads a user catalog. This closes both
gaps in `crates/bhtune-cli/src/commands/template.rs`.

- **`template import` auto-detects JSON vs. TOML by sniffing content, not extension or a
  try-then-fallback.** `looks_like_json_object` checks whether the file's content, ignoring
  leading whitespace, starts with `{`; if so it's parsed as a single JSON template (the
  existing `import_one`, unchanged hard-fail-on-name-collision behavior); otherwise it's
  parsed as a TOML catalog via `bhtune_core::template::parse_catalog` (the new
  `import_catalog`). Chosen over "try JSON, fall back to TOML on failure" specifically so a
  malformed file of either format gets a format-specific, useful parse error rather than
  always surfacing the TOML parser's complaint about content the user actually meant as
  JSON — legitimate TOML catalog files can never start with a bare `{` at the document root
  (not valid top-level TOML), so the heuristic never misclassifies real content of either
  format.
- **`import_catalog` is best-effort, deliberately unlike `import_one`.** A single-JSON-
  template import still hard-fails on a name collision (unchanged — it's one deliberate
  template, so a collision is a mistake to fix). A multi-template TOML catalog import
  instead skips any colliding name and reports both what was imported and what was skipped,
  because the expected workflow is re-importing an updated community catalog file that
  overlaps with templates already present — the useful outcome is "add what's new," not
  "fail because some of this was already here." An empty catalog (`template = []`) is its
  own message, not an error.
- **`template export --format <json|toml>`** (new `TemplateFileFormat` `ValueEnum` in
  `args.rs`, default `json`, so the existing default behavior is unchanged for anyone not
  passing the flag). The TOML path is `bhtune_core::template::to_catalog_toml(vec![row.
template])` — a single-template file is just a one-entry catalog, so it round-trips
  through the exact same `parse_catalog` used everywhere else, and the output is a
  `[[template]]` block ready to paste into a contribution PR (the export → annotate → PR
  loop `template-catalog`'s design section had planned for since before this todo existed).
- **`template delete <name>`** — there was previously no way to remove a template at all.
  Looks the template up by name (the existing "no template named" error if missing), then
  calls `bhtune_db::models::DcsTemplateRow::delete`: `Ok(true)` deletes and prints an
  origin-specific note for `Builtin`/`Catalog` templates (both will silently reappear on the
  next startup unless also removed from their source — the embedded catalog for `Builtin`,
  which only a new release can change, or the user's `templates.toml` for `Catalog` — so the
  CLI says so up front rather than letting a confused re-appearance be discovered later);
  `Ok(false)` (a same-process TOCTOU race — something else deleted the row between the
  lookup and the delete call) is reported as "already deleted" rather than treated as a bug;
  `Err(DbError::TemplateInUse)` becomes a friendly "still referenced by one or more saved
  loops" message instead of a raw SQL error, deleting nothing.
- **`template list` gained a VERSIONS column** (between ORIGIN and PROPORTIONAL),
  formatting `versions.join(", ")` or `"-"` for a template with none recorded.
- **`import_one` (the single-JSON-template path) now calls `DcsTemplate::validate()`
  itself**, before the name-collision check. It parses with plain `serde_json::from_str`
  rather than going through `parse_catalog`, so — unlike `import_catalog`, which inherits
  validation for free from `parse_catalog` — it never got the validation `template-catalog`
  added to `DcsTemplate` in the first place; this closes that gap explicitly.
- **`bhtune-cli` gained a `sqlx` dev-dependency** (not a production one — this crate's
  non-test code still only ever talks to `bhtune-db`'s repository API) purely so the
  `delete`-still-referenced test can insert a raw `loops` row via SQL, mirroring the exact
  pattern `bhtune-db`'s own `tests/schema.rs` already uses for the identical FK check; there
  is no `LoopRow::insert` yet (loop-saving is `cli-commands` scope, not this todo's).

**Testing approach.** 22 tests in `commands/template.rs` (up from 10) cover: TOML single-
template export round-tripping through import under a renamed identity; a multi-template
TOML catalog import adding new entries while skipping ones that already exist by name (and
the all-new/all-skipped edge cases separately); an empty-catalog import being a no-op;
invalid-TOML-catalog import producing a TOML-specific error (proving the content-sniffing
heuristic routes correctly, alongside the pre-existing invalid-JSON-import test); a JSON
template that parses but fails `validate()` being rejected without ever reaching the
database; `delete` succeeding for `User`/`Builtin`/`Catalog`-origin templates (each
exercising its own note branch); `delete` failing cleanly on an unknown name and on a
template still referenced by a
loop (the latter inserting a real `loops` row, per the `sqlx` dev-dependency note above); and
`list` formatting a template with no recorded `versions` as `"-"`. One further test added to
`bhtune-db/tests/schema.rs` (`dcs_template_delete_reports_a_non_database_error_as_query_not_
template_in_use`, using a closed pool to force a non-`Database` `sqlx::Error`) closes a
coverage gap in `DcsTemplateRow::delete`'s own classification logic that predates this todo.
Two branches remain deliberately untested and documented in place, consistent with this
project's existing accepted-gap precedent (e.g. `safety-db-restore`'s exclusivity-check
residual race): `delete`'s own same-process TOCTOU branch, and the CLI-level passthrough for
a non-`TemplateInUse` `DbError` from `DcsTemplateRow::delete` — both require a database error
to occur between two calls that share one connection pool with no `.await` yield point
between them, which is not deterministically constructible without fault-injection seams
this project has no other use for. `cargo llvm-cov` confirms these are the only three lines
(across those two branches) left uncovered in the entire file.
