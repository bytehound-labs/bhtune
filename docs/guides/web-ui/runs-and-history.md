---
sidebar_position: 3
---

# Runs and history

The History page at `/runs` lists stored tunes with filters and pagination. Full-mode filter
selections and page offsets are encoded in the URL, so reloads and browser back/forward
navigation restore the same view. Opening a run and returning to History preserves that query.
Invalid filter values are reset with an inline explanation. Demo keeps pagination but does not
accept Full-only filter parameters. The list and tune form fit viewports at and below 1024
pixels; the history table scrolls inside its own container when it is wider than the screen.

A run detail page at `/runs/:id` shows the live or completed trend, configuration, results,
notes, initial readings, and PID audit information. The run ID in its heading can be copied;
the selected OPC ItemID in the tag browser has the same action. Each copy control reports
success only after the clipboard accepts the value and explains when copying is unavailable.

## History list

{/* web-ui-screenshot: full-history */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-history.png?v=607cc449bff3">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-history.png?v=607cc449bff3" alt="BHTune Full mode History page with filterable tune-run rows" />
  </a>
  <figcaption>History provides filterable, paginated access to stored runs; open a row for its complete detail and audit trail.</figcaption>
</figure>

Demo history is private to the anonymous browser session that created it. A different browser
profile receives the same `404` for another session's run ID.

{/* web-ui-screenshot: demo-history */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-history.png?v=1419b2b6bea7">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-history.png?v=1419b2b6bea7" alt="BHTune Demo mode visitor-private History page" />
  </a>
  <figcaption>Demo history uses the same list shape while the server scopes every row to the current anonymous session.</figcaption>
</figure>

## Live run detail

While a run is active, the trend chart streams persisted samples over Server-Sent Events. The
initial PV/MV readings appear before the first MRFT sample, and a **Cancel run** action follows
the same abort-and-restore path as the CLI. Simulator timestamps use the configured fixed
poll step; live OPC DA timestamps use monotonic elapsed time projected onto the run start.

{/* web-ui-screenshot: full-run-live */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-live.png?v=65490d12af2c">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-live.png?v=65490d12af2c" alt="BHTune Full mode active run detail with live PV and MV trend and cancellation control" />
  </a>
  <figcaption>Live run detail combines progress, current measurements, the streaming trend, and the safety action to cancel.</figcaption>
</figure>

{/* web-ui-screenshot: demo-run-live */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-run-live.png?v=da8869d73ab1">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-run-live.png?v=da8869d73ab1" alt="BHTune Demo mode active simulator run detail with live trend" />
  </a>
  <figcaption>Demo runs use the same live trend and cancellation workflow, but cannot access live-plant actions.</figcaption>
</figure>

## Completed run detail

Once a run completes, the page hands the chart to the stored samples and shows the calculated
results before the trend. The trend adds presentation-only initial-reading and restored-MV
boundary points; exports contain the persisted sample series. Invalid results remain visible
with a reason and cannot be written as PID constants.

The run-detail page announces outcome changes through a polite screen-reader status region
without announcing every streamed sample. The live progress panel shows the current tick, PV,
MV, and cycle counts. The trend legend uses **PV** and **Commanded MV**; plotted values are not
converted and engineering units are not inferred. The Full-mode Summary retains the recorded
run tag. There is no point selector, recorded-tag caption, or repeated measurement readout
beneath the chart. A hidden description lets screen readers read the figure's plotted point
count, time span, axes, and observed PV/MV ranges; it does not add measurements or controls.

{/* web-ui-screenshot: full-run-complete */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-complete.png?v=8482d63361c2">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-complete.png?v=8482d63361c2" alt="BHTune Full mode completed run detail with calculated results, trend, summary, notes, and audit sections" />
  </a>
  <figcaption>Completed detail promotes calculated results above the trend and keeps the full run evidence below in collapsible sections.</figcaption>
</figure>

{/* web-ui-screenshot: demo-run-complete */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-run-complete.png?v=edcf42e130f4">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-run-complete.png?v=edcf42e130f4" alt="BHTune Demo mode completed simulator run detail with results and diagnostics" />
  </a>
  <figcaption>Demo completion shows the same simulator results and diagnostics without any write-back controls.</figcaption>
</figure>

The page's independently collapsible sections include Summary, Notes, Test configuration,
Initial readings, PID change history, and Sampling diagnostics. When a live OPC DA run stored a
gateway compatibility snapshot, Summary shows it as a badge: full, partial, unknown, or
incompatible. Simulator runs and older runs have no badge. See
[OPC gateway compatibility](../opc-gateway-compatibility.md). Sampling adequacy is advisory:
adequate means at least six observed samples per measured period, marginal means fewer than six,
and not assessed means no usable period exists.

## Reviewing PID actions

Eligible completed OPC DA runs show **Review & write** for each calculated response level. The
review popup names the loop tag, response level, snapshotted parameter labels, exact destination
tags, and values. Apply closes the popup while the request continues in the background; physical
write and readback failures appear in the page alert and audit table.

The table and write popup use the same backend-generated controller targets: Yokogawa displays
one decimal place, while the other built-ins use three significant digits. The run retains its
template's precision policy even after catalog edits. JSON keeps raw calculated values in their
original fields and exposes controller-ready values separately. A result whose active term would
round to zero is **Unwritable**, with a reason and disabled write action; its raw calculation
status is unchanged.

{/* web-ui-screenshot: full-pid-review */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-pid-review.png?v=d9c6b9903ca5">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-pid-review.png?v=d9c6b9903ca5" alt="BHTune PID review modal showing the exact response level, destination tags, and values before a write" />
  </a>
  <figcaption>The safety review is the last visual confirmation before calculated PID values are sent to a live controller.</figcaption>
</figure>

The newest successful write offers **Restore previous values** through the same popup. The
restore review and the audit show the recorded values without applying template rounding.
Both actions are disabled with a reason unless the run is finished, used OPC DA, has all PID tags,
and recorded its original server and bridge connection. Export CSV/JSON, **Delete tune**, and
**Duplicate this run** are available from the completed detail page. Delete tune opens a styled
confirmation dialog before it removes the run's persisted samples, calculated results, and PID
write/audit history; the deletion cannot be undone through the UI. While the request is pending,
the dialog's confirm, cancel, close, backdrop, and Escape dismissal paths are locked. A failed
request leaves the dialog open with an inline retryable error and leaves the run in history;
retrying uses the same dialog and removes the run only after a successful response, then returns
to `/runs`.

{/* web-ui-screenshot: full-history-delete-confirmation */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-history-delete-confirmation.png?v=573fe33a8e4e">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-history-delete-confirmation.png?v=573fe33a8e4e" alt="BHTune failed completed-run deletion confirmation with an inline retry error" />
  </a>
  <figcaption>Run deletion is a retryable, styled confirmation: the failed attempt does not change history, while the successful retry removes the run and navigates back to History.</figcaption>
</figure>
