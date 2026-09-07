---
sidebar_position: 3
---

# Runs and history

The History page at `/runs` lists stored tunes with filters and pagination. A run detail page
at `/runs/:id` shows the live or completed trend, configuration, results, notes, initial
readings, and PID audit information.

## History list

{/* web-ui-screenshot: full-history */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-history.png?v=7a297945e8b4">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-history.png?v=7a297945e8b4" alt="BHTune Full mode History page with filterable tune-run rows" />
  </a>
  <figcaption>History provides filterable, paginated access to stored runs; open a row for its complete detail and audit trail.</figcaption>
</figure>

Demo history is private to the anonymous browser session that created it. A different browser
profile receives the same `404` for another session's run ID.

{/* web-ui-screenshot: demo-history */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-history.png?v=13ae4427d5d0">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-history.png?v=13ae4427d5d0" alt="BHTune Demo mode visitor-private History page" />
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
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-live.png?v=5bbe11a6819c">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-live.png?v=5bbe11a6819c" alt="BHTune Full mode active run detail with live PV and MV trend and cancellation control" />
  </a>
  <figcaption>Live run detail combines progress, current measurements, the streaming trend, and the safety action to cancel.</figcaption>
</figure>

{/* web-ui-screenshot: demo-run-live */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-run-live.png?v=4bb97d1412d8">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-run-live.png?v=4bb97d1412d8" alt="BHTune Demo mode active simulator run detail with live trend" />
  </a>
  <figcaption>Demo runs use the same live trend and cancellation workflow, but cannot access live-plant actions.</figcaption>
</figure>

## Completed run detail

Once a run completes, the page hands the chart to the stored samples and shows the calculated
results before the trend. The trend adds presentation-only initial-reading and restored-MV
boundary points; exports contain the persisted sample series. Invalid results remain visible
with a reason and cannot be written as PID constants.

{/* web-ui-screenshot: full-run-complete */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-complete.png?v=b0a8a6e0c61c">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-complete.png?v=b0a8a6e0c61c" alt="BHTune Full mode completed run detail with calculated results, trend, summary, notes, and audit sections" />
  </a>
  <figcaption>Completed detail promotes calculated results above the trend and keeps the full run evidence below in collapsible sections.</figcaption>
</figure>

{/* web-ui-screenshot: demo-run-complete */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-run-complete.png?v=567280ebc678">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-run-complete.png?v=567280ebc678" alt="BHTune Demo mode completed simulator run detail with results and diagnostics" />
  </a>
  <figcaption>Demo completion shows the same simulator results and diagnostics without any write-back controls.</figcaption>
</figure>

The page's independently collapsible sections include Summary, Notes, Test configuration,
Initial readings, PID change history, and Sampling diagnostics. Sampling adequacy is advisory:
adequate means at least six observed samples per measured period, marginal means fewer than six,
and not assessed means no usable period exists.

## Reviewing PID actions

Eligible completed OPC DA runs show **Review & write** for each calculated response level. The
review popup names the loop tag, response level, snapshotted parameter labels, exact destination
tags, and values. Apply closes the popup while the request continues in the background; physical
write and readback failures appear in the page alert and audit table.

{/* web-ui-screenshot: full-pid-review */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-pid-review.png?v=459ff13818c8">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-pid-review.png?v=459ff13818c8" alt="BHTune PID review modal showing the exact response level, destination tags, and values before a write" />
  </a>
  <figcaption>The safety review is the last visual confirmation before calculated PID values are sent to a live controller.</figcaption>
</figure>

The newest successful write offers **Restore previous values** through the same popup. Both
actions are disabled with a reason unless the run is finished, used OPC DA, has all PID tags,
and recorded its original server and bridge connection. Export CSV/JSON, Delete tune, and
Duplicate this run are available from the completed detail page.
