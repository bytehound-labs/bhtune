---
sidebar_position: 2
---

# Starting a tune

The **New Tune** page at `/runs/new` combines connection, tag mapping, test parameters,
simulator settings, notes, and optional automatic PID write-back. The server validates the
request again when the run starts; the form's disabled states and validation messages are
convenience, not a safety boundary.

## Simulator form

Choose **Simulator** when learning the workflow or checking a template's PID-unit conversion
without a plant connection. The form keeps the same layout but disables the OPC server, bridge,
tag, quality, timeout, and automatic write-back controls because the in-process simulator cannot
use them. The template remains enabled because it still controls the result convention shown
after the run.

{/* web-ui-screenshot: full-tune-simulator */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-tune-simulator.png?v=13982ceeafce">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-tune-simulator.png?v=13982ceeafce" alt="Full mode Simulator New Tune form with live-plant fields disabled" />
  </a>
  <figcaption>Simulator mode disables only controls that require live equipment; process ranges, direction, MRFT parameters, and the template remain active.</figcaption>
</figure>

The **Test parameters** section shows process-type defaults for cycles to skip, cycles to count,
and noise protection. Changing process type replaces those three values; **Reset process
defaults** restores them without changing unrelated tune settings. Installation-wide MRFT
delay, poll interval, operation timeout, whole-run timeout, and restore timeout are managed on
the [Configuration](templates-and-configuration.md#configuration) page.

## OPC DA connection and mapping

Choose **OPC DA** to expose the Bridge host, OPC DA server ProgID, Tag name, and Notes fields.
**Browse servers** discovers registered ProgIDs on the configured gateway. **Browse tags** opens
a one-level-at-a-time tree after a ProgID is present. The browser can expand dotted or
slash-separated namespaces, test-read a selected item, and preserve the current path when
reopened.

{/* web-ui-screenshot: full-tune-opc */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-tune-opc.png?v=9a10ac2832ab">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-tune-opc.png?v=9a10ac2832ab" alt="Full mode OPC DA New Tune form showing connection, test parameters, and Loop mapping" />
  </a>
  <figcaption>OPC DA mode exposes the live connection and mapping controls that the simulator disables.</figcaption>
</figure>

{/* web-ui-screenshot: full-opc-server-picker */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-opc-server-picker.png?v=1da29a044fce">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-opc-server-picker.png?v=1da29a044fce" alt="BHTune OPC DA server discovery modal listing available server ProgIDs" />
  </a>
  <figcaption>Server discovery is on demand and fills the ProgID field when an engineer selects a listed OPC DA server.</figcaption>
</figure>

{/* web-ui-screenshot: full-opc-tag-browser */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-opc-tag-browser.png?v=12cac28094d8">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-opc-tag-browser.png?v=12cac28094d8" alt="BHTune hierarchical OPC tag browser with a selected tag and a successful quality read" />
  </a>
  <figcaption>The tag browser expands one level at a time and reads the originally selected item before it is mapped into the loop.</figcaption>
</figure>

### Template suffix replacement

Selecting any browsed item does not blindly append `.PV`. BHTune reads the original item first
for OPC quality verification, then replaces everything after the final `.`, `!`, or `/` with
the active template's `process_variable_suffix`, preserving the preceding path and separator.

For example, with a template whose process-variable suffix is `PV`:

```text
Browsed item:  Area01.FIC101.OUT
Tag name:      Area01.FIC101.PV
```

This is final-component replacement, not string concatenation. The same separator-aware rule
means `FCS0201/Control/OUT` becomes `FCS0201/Control/PV`, and a tag with a `!` separator keeps
that separator. The backend applies the same derivation when the run starts, so the preview is
not a second source of truth.

{/* web-ui-screenshot: full-opc-tag-applied */}
<figure>
  <a href="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-opc-tag-applied.png?v=291c5d4db318">
    <img src="https://bytehound-labs.github.io/bhtune/generated/web-ui/full-opc-tag-applied.png?v=291c5d4db318" alt="BHTune New Tune form showing Area01.FIC101.PV after selecting Area01.FIC101.OUT in the OPC browser" />
  </a>
  <figcaption>After selection, the Tag name and Loop mapping preview show the template's PV suffix applied to the browsed path.</figcaption>
</figure>

The **Loop mapping** section is the single place to inspect the effective tag set. Tag rows use
Template tag or Custom tag sources. Direction and range rows additionally support Fixed value.
Changing the base Tag name or template resets Custom tag sources and preserves Fixed value
choices. Use a row's **Reset** or **Reset all mapping overrides** action to return to
template-derived values.

The browser's **Read selected tag** action displays value and quality. `Good` quality proceeds
immediately. `Uncertain` or `Bad` quality requires an explicit choice to select another item or
continue; starting a tune still applies the server's normal quality safeguards.

## Drafts, notes, and automatic PID writes

Every editable field except Notes is saved as the app-wide New Tune draft, including values
belonging to the inactive driver. Notes intentionally start blank after reload so operator
context is not copied into another tune. **Duplicate this run** takes precedence over the saved
draft and newest-run fallback; **Reset to defaults** replaces the draft with built-in defaults.

Notes are optional run metadata and can be edited or cleared from run detail. Automatic PID
write-back is available only for an eligible OPC DA run and requires explicit confirmation;
reviewing or writing calculated results after completion is documented in
[Runs and history](runs-and-history.md).
