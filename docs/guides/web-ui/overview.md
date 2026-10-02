---
sidebar_position: 1
---

# Web UI overview

BHTune's browser application is served by `bhtune-server`. The Full and Demo modes use the
same navigation and run-history concepts, but the server decides which capabilities are
available before the frontend renders them. Full mode can connect to OPC DA and change
templates, configuration, and PID constants. Demo mode is simulator-only, visitor-private,
and intentionally omits live-plant controls.

## The application shell

The header provides the primary navigation, a Catppuccin light/dark theme toggle, and a
health indicator. The health indicator confirms that the BHTune HTTP service responds; it
does not test an OPC DA gateway or prove that a controller is reachable.

## Keyboard and screen-reader support

Form controls have associated labels, hints, and inline validation messages. Modal dialogs
move focus into the dialog, keep keyboard focus within it, and return focus to the control
that opened them when they close. Escape, the close button, and the backdrop dismiss a
dialog when no operation is pending.

The OPC tag browser uses a keyboard-operable tree: Up and Down move between visible nodes,
Home and End move to the first and last visible nodes, Left collapses or moves to the parent,
and Right expands a branch or moves to its first child. Enter or Space selects an ItemID;
Right expands branch-and-item nodes without changing their exact ItemID. The search field
offers its indexed matches as an accessible listbox.

{/* web-ui-screenshot: full-tune-simulator */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-tune-simulator.png?v=2046aaf9cebd">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-tune-simulator.png?v=2046aaf9cebd" alt="BHTune Full mode New Tune page with the Simulator driver selected" />
  </a>
  <figcaption>The New Tune page is the default landing screen. Simulator-only runs keep the form layout stable and disable controls that require live OPC DA equipment.</figcaption>
</figure>

The screenshots in this guide are generated from the production SPA with fixed, sanitized
fixtures. They illustrate the current layout and workflow, but the adjacent prose is the
operational reference and remains complete when images are disabled. Select any image to open
the full-size Pages asset.

## Full mode and Demo mode

Full mode exposes the complete local/operator workflow:

- check tune readiness from the New Tune page without starting a run or writing values;
- start Simulator or OPC DA runs;
- browse OPC DA servers and tags;
- edit user-owned templates;
- change global quality and retention settings;
- review and write PID values after a run.

Demo mode exposes only bounded simulator tuning and visitor-owned run history. The Demo notice
explains the fixed policy and simulator boundary in the browser; it is not an authentication
system or a route-level substitute for the server's Demo enforcement. The Full-mode readiness
action and `POST /api/runs/preflight` endpoint are not available in Demo mode.

{/* web-ui-screenshot: demo-tune */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-tune.png?v=9ce922cb9355">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/demo-tune.png?v=9ce922cb9355" alt="BHTune Demo mode simulator tune form with the persistent Demo policy notice" />
  </a>
  <figcaption>Demo mode keeps simulator controls visible while omitting live-plant actions. The persistent notice states the boundary, history limit, and session lifetime.</figcaption>
</figure>

See [Public simulator demo](../public-simulator-demo.md) for privacy, quotas, deployment
requirements, and the security boundary. Use the other pages in this section for task-focused
instructions:

- [Starting a tune](starting-a-tune.md)
- [Runs and history](runs-and-history.md)
- [Templates and configuration](templates-and-configuration.md)
