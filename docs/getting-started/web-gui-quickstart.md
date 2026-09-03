---
sidebar_position: 3
---

# Web GUI quickstart

`bhtune-server` is a single binary that serves both a REST + Server-Sent-Events API and the
built React frontend — no separate Node process, reverse proxy, or Docker container required on
the target host. This walks through running a complete tune from a browser, against the
built-in simulator.

## Build and run

```sh
pnpm install
pnpm --filter bhtune-frontend run build   # embeds frontend/dist/ into the server binary
cargo run --release --bin bhtune-server
```

```text
bhtune-server listening on http://127.0.0.1:8787
```

Open that URL in a browser. If you skip the frontend build, the server still starts and serves
its API — every `/api/*` route works — but any other path returns a clear
`503 the web UI has not been built yet` instead of a blank page, naming the exact command to
run.

The header's theme button switches between Catppuccin light and dark palettes. The selected palette is
remembered by the browser.

The header also shows whether the BHTune HTTP service is reachable. Its status includes
screen-reader text and a tooltip; a healthy server indicator does not test OPC DA connectivity.

### Restricted Demo mode

A server configured for Demo mode exposes a simulator-only browser experience. The frontend
loads the server capability document before it queries run history or renders mode-sensitive
routes. That request lazily sets the host-only anonymous Demo cookie but does not create a
database row; storage begins only when the browser starts its first accepted tune. Config, OPC
browsing, notes, and PID write-back are absent; templates are limited to the built-in read-only
catalog. Demo run history, detail, streaming, cancellation, export, and deletion use the same
`/api/runs` paths as Full mode. Starting a tune sends only the normalized simulator fields shown
by the Demo form, with a fixed safe tag, bounded simulator ranges and timing values, and a
controller direction that must provide negative feedback for the selected positive or negative
process gain. The Demo defaults are a 0–100 PV/MV range with initial values of 50, gain 1.0,
time constant 0.5 seconds, dead time 1 second, zero noise, relay amplitude 10%, one skipped
cycle, two counted cycles, and zero seconds of noise protection.

Demo policy limits and simulator timing are fixed application-owned values rather than public
configuration controls; a deployment configuration may only declare the exact contract.
Each run uses the stable **Simulator demo** identity. Demo history belongs to the anonymous browser session that created it. Another browser profile
or private window cannot list or open those runs. The Demo form draft is stored only in that
browser and expires 24 hours after its last edit. A usage-limit message advises waiting before
retrying (`429`), while temporary service-capacity failures (`503`) advise retrying after a
short pause. Full mode keeps the complete behavior described below.

`bhtune-server` has no command-line flags of its own (unlike `bhtune`, it doesn't use `clap`
yet). Every setting — including which port it binds — is resolved from a config file or
environment variable, the same way the CLI resolves its own flags:

```sh
BHTUNE_BIND=0.0.0.0:8080 cargo run --release --bin bhtune-server   # bind elsewhere, e.g. for LAN access
```

Binding anywhere other than `127.0.0.1` exposes the server to your network with **no
authentication** — see [Safety](../guides/safety.md#network-exposure) before doing this outside
a trusted, isolated OT network. Authentication is a planned, not yet shipped, feature (see the
roadmap in the main [README](https://github.com/bytehound-labs/bhtune#readme)).

## Frontend development mode

If you're changing the frontend itself, run the Vite dev server alongside a running
`bhtune-server` instead of rebuilding on every change — Vite proxies `/api/*` requests through
to the real server so both stay in sync:

```sh
cargo run --bin bhtune-server &     # first, in one terminal
pnpm --filter bhtune-frontend run dev   # then, in another -- hot-reloads on save
```

The Vite development server binds all local interfaces and allows the `asus` hostname, so a
second host on the trusted local network can open `http://asus:5173`. Frontend edits are
deployed through hot module reload after each save; restart `bhtune-server` after Rust or API
changes. The proxy keeps browser API calls same-origin to the Vite page; Full mode accepts
that development flow while continuing to reject cross-site browser mutations. The
development server and API have no authentication, so do not expose them beyond a trusted
network.

## Run a tune

1. **Tune** (`/runs/new`) — the app's default landing page, and the first item in the header
   nav. One form covering the per-tune settings for `bhtune tune`: connection (which driver —
   OPC DA or the simulator), tag mapping, test parameters (process type, controller type,
   relay amplitude, cycles), simulator parameters (gain/time constant/dead time/noise, when
   the simulator driver is selected), and automatic PID settings. Submitting POSTs to the same
   `/api/runs` endpoint the CLI's `bhtune-server` mode exposes — there's exactly one API,
   used by both the browser and any script that wants to drive a run over HTTP directly.
   Multiple tunes can run at the same time. PID writes and restores remain exclusive with
   active tunes because they modify live controller values directly.
   The **Start tune** and **Cancel** actions remain at the top of the page while you configure
   the tune.

   - Switching the driver to **Simulator** greys out every field the simulator genuinely
     ignores (OPC server ProgID, bridge host, tag name, automatic PID settings, and quality
     options) rather than hiding them, so the form doesn't reflow and the greyed field itself
     explains what the simulator doesn't use. The template stays enabled intentionally: the
     simulator ignores its DCS tag mappings, but its PID type and unit conventions still format
     calculated results (for example, Yokogawa uses proportional band while the other built-in
     templates use gain). PV/MV ranges, controller direction, and every engine parameter also
     stay enabled because the simulator needs them.
   - Test parameters show concrete **Process defaults** for cycles to skip, cycles to count, and
     noise protection based on the selected Process type. Changing Process type replaces all
     three values; **Reset process defaults** restores those values without resetting the rest
     of the form. These are process-type defaults rather than DCS/PLC template settings.
   - Switching to **OPC DA** reveals a **Browse servers** button next to the ProgID field.
     It opens an on-demand picker populated by the bridge gateway, listing every OPC DA server
     registered on it as clickable buttons — no need to already know (or spell correctly) a
     ProgID like `Matrikon.OPC.Simulation`. A **Browse tags** button next to the Tag name field
     (enabled once a ProgID is entered) opens a lazily-expanding tag tree fetched one level at
     a time from the gateway. Already-loaded levels are reused when you revisit an expanded
     branch. Hierarchical servers such as Yokogawa CSHIS can be expanded
     through nested controller/block levels until their PV leaves; dotted and slash-separated
     item IDs are supported. The first node is selected automatically when the tree loads, and
     the selection panel stays in place while browsing. The main form's collapsible **Loop
     mapping** section is the single place to inspect the complete tag set the active template
     derives from the selected PV and the direction/range inputs used for the tune. Each row
     shows its effective value and source. Tag mappings use **Template tag** or **Custom tag**;
     direction and range mappings use **Template tag**, **Custom tag**, or **Fixed value**. Switching to a
     custom tag starts with the template-derived value; fixed direction/range values must be
     entered explicitly. Each mapping row is a labeled group, and its source choices are exposed
     as an accessible pressed-button set. Use a row's **Reset** button or **Reset all mapping overrides** to return to template/live
     values. Simulator direction and ranges are kept separately from OPC fixed overrides. The
     browser itself stays focused on browsing and testing the selected PV tag. It offers a
     **Read selected tag** action showing a live value
     and its quality. Double-clicking a leaf selects it; double-clicking a branch expands or
     collapses it. Selecting **Select tag** writes the selected tag with its final component
     replaced by the active template's process-variable suffix into the Tag name field. Changing
     templates likewise replaces the final component with the new template's process-variable
     suffix, regardless of what the previous component was, preserving the rest of the tag path.
     Clicking **Select tag** performs a fresh read of the original selected item (before suffix
     replacement) and proceeds immediately only for `Good` OPC quality; `Uncertain` or `Bad`
     quality opens a warning with choices to select a different tag or proceed anyway. Proceeding
     only selects the item; tune execution still applies its live-reading quality safeguards.
     Changing the base Tag name — by editing it, selecting a browser tag, or switching templates
     — resets every **Custom tag** selector to **Template tag** and clears custom tag values,
     including custom direction/range read tags. **Fixed value** direction and range selections
     and values remain unchanged.
     The **Config** page controls whether `Uncertain` readings are accepted during tuning;
     they are accepted by default, while `Bad` quality is always rejected.
     Reopening the browser expands the available path to the current Tag name, selects that node,
     and scrolls it into view; an unavailable tag falls back to the root level.
   - A **Notes** field records optional operator context, observations, or follow-up actions.
     Notes are included when the run starts and can be edited or cleared from the run detail
     page while the run is active or after it finishes.
   - Every field except **Notes** is autosaved to the server as the app-wide New tune draft,
     including OPC values that are temporarily inactive while **Simulator** is selected. The
     draft follows you across browsers and machines rather than living in `localStorage`.
     Notes are intentionally left blank after a reload so one run's operator context is not
     copied into another. On an installation without a saved draft, the form quietly falls back
     once to the newest run's settings or the built-in defaults; this normal first-use state
     does not display an error. **Duplicate this run** takes precedence over both sources, and
     **Reset to defaults** replaces the saved draft with the built-in defaults. Connection, Test
     parameters, Loop mapping, Simulator parameters, and Automatic PID settings are independently
     collapsible and open by default. Controls that do not apply to the selected driver stay
     visible but disabled with an explanation.
   - Installation-wide MRFT timing and safety values are managed on the **Config** page, not on
     this form: MRFT delay, poll interval, whole-run timeout, driver-operation timeout, and
     restore timeout. They are stored under `[tuning]` in `bhtune.toml`, apply to future tune
     preparations, and are frozen into each run when it starts. The three process-dependent
     defaults in **Test parameters** remain per-tune values.

2. **Run detail** (`/runs/:id`) — while a run is in progress, a live PV/MV trend chart updates
   in real time over Server-Sent Events (`GET /api/runs/:id/stream`), with line-only PV/MV series
   alongside the current
   relay switch count and cycles remaining. The initial PV/MV snapshot appears as soon as the
   server records it, before the first MRFT sample, so the chart does not wait for a complete
   relay tick to become visible. Independent OPC DA startup values are collected in one
   batched read; a setpoint is read separately only when the original mode is Auto. Simulator
   sample timestamps advance by the configured fixed poll step, matching the FOPDT process time
   rather than host scheduler timing, so repeated simulator runs retain the same trend timing and
   PID calculations across machines. Live OPC DA timestamps instead use actual monotonic elapsed
   time projected onto the run's UTC start, making clock adjustments irrelevant without hiding
   real scheduling or driver delays. BHTune is not a hard-real-time controller, so the host and
   gateway still need to remain responsive. The trend ends with a terminal point at the original
   MV after the run restores the loop. Short runs keep
   their first point at the left edge by reserving 12 configured poll intervals of x-axis
   horizon; unused future space stays blank until the trend is long enough to fit normally. The
   initial-reading and restored-MV boundary markers are presentation-only and do not alter
   persisted samples or CSV/JSON exports. A **Cancel** button stops the run early (the same
   Ctrl+C-triggered abort-and-restore path the CLI uses — see
   [Safety](../guides/safety.md#cancellation)). Once calculated results exist, the same page
   promotes them directly below the heading and before the trend:
   - The **Calculated results** panel is the primary post-tune action area. Each
     Aggressive/Moderate/Sluggish row has a **Review & write** button that works independently
     of any `--write-pid` choice made before the run started. The safety review modal names the
     loop tag, response level, snapshotted parameter labels, exact destination tags, and exact
     values before anything is sent. It opens as a centered viewport popup. Confirming an Apply
     closes the popup immediately while BHTune writes and verifies the values in the background;
     successful writes stay silent, while transport failures or failed physical writes/readbacks
     appear in a page-level alert. The same popup component is used by the OPC server and tag
     browsers.
     When no results exist, the panel stays in its lower diagnostic position and explains that
     no results were calculated.
   - A mutable **Notes** field with **Save notes** and **Clear notes** actions. Notes are
     metadata, so editing them does not interrupt an active tune.
   - A **PID change history** table of every PID change this tune has made, each with a pre-write
     readback, a post-write readback, and a rollback status. The newest successful write
     shows a **Restore previous values** button that opens the same safety review modal and
     lists the recorded pre-write values before restoring them. Confirming a restore closes the
     popup immediately while BHTune works in the background; successful restores stay silent,
     while transport failures or failed physical restores/readbacks appear in a page-level alert.

     Both buttons are disabled — with the reason shown as text, never a silent, unexplained
     grey button — unless the run is finished, used the OPC DA driver, has PID constant tags
     configured, and recorded which OPC server/bridge host it connected to.

   - **Export CSV**/**Export JSON** download links and a **Delete tune** button (with a
     confirmation prompt — deleting a tune also removes its recorded measurements and
     results, and cannot be undone).
   - A **Duplicate this run** button, returning to the New tune form prefilled from this run's
     tune settings instead of the newest run's; Notes starts blank. It preserves the original
     Template tag, Custom tag, and Fixed value mapping sources, including template-derived
     direction and range values that were stored as null or omitted fields.

   - The **Calculated results** table uses the constant names and converted values from the
     run's snapshotted template. A Yokogawa run therefore shows `P`, `I`, and `D` instead of
     the engine's intermediate `Kp`, `Ti`, and `Td` columns. The derivative column remains
     visible for PI runs and shows `0`, the explicit value used to clear stale derivative action.
     A result whose amplitude, period, or converted PID values are zero, non-finite, or otherwise
     unusable is shown as **Invalid** with a reason and no numeric values; its calculated-result
     write action is disabled.
   - The collapsed **Sampling diagnostics** section reports sampling adequacy: **adequate** means
     at least six observed samples per measured period, **marginal** means fewer than six, and
     **not assessed** means no usable finite period was available. This is an advisory signal, not
     an automatic block on a valid result. Detailed sample-gap and successful operation-latency
     diagnostics remain available through the CLI, API, and structured logs.
   - Run-detail sections are independently collapsible. Calculated results, Trend, Summary,
     Notes, Test configuration, Initial readings, and PID change history start expanded so the
     main result and PID audit information is immediately visible. Sampling diagnostics starts
     collapsed because it is advisory. Detailed MV command/readback evidence is available through
     `bhtune history show`, the run-detail API, and structured logs when deeper support or safety
     analysis is needed.

   Detailed polling timing diagnostics remain available through `bhtune history show`, the
   run-detail API, and structured logs, but are not part of the normal web run-detail view.

3. **History** (`/runs`) — every past tune, shown by **Tag name** and filterable by outcome and
   process type, with the same detail view available for any completed run — not just the one
   you just started.
4. **Templates** (`/templates`) — the four built-in DCS/PLC templates are listed on first
   launch (seeded automatically into the database). Open one to see its full tag-suffix
   mapping, or create a new one instead of hand-editing a TOML file.

## Explore the API directly

Every route is documented at `/api/docs` (an interactive Scalar UI generated from the same
OpenAPI 3.1 spec the frontend's TypeScript client is generated from — the two can never drift
apart), and the raw spec is at `/api/openapi.json`. A minimal health check:

```sh
curl http://127.0.0.1:8787/api/health
# {"status":"ok","version":"<application-version>"}
```

The web GUI displays this application version beside its connection status in the header.

Starting a tune over HTTP directly (no browser) needs the same per-tune fields as the CLI's
`tune`/`simulate` commands — see `/api/docs` for the full request schema, including the extra
range/direction fields the simulator driver requires that a real OPC DA driver would instead
read live from the DCS. The global `[tuning]` settings are read from the server's configuration
when the run is prepared.

## Next steps

- [CLI quickstart](cli-quickstart.md) — the same tuning engine, scriptable for scheduled/
  unattended runs.
- [Safety](../guides/safety.md) — cancellation, quality enforcement, and write-back rollback,
  all shared between the CLI and the server.
- [Configuration](../reference/config.md) — TOML-backed global tuning, quality, and retention policies.
- [DCS/PLC templates](../dcs-templates.md) — the tag-mapping system behind the Templates screen.
