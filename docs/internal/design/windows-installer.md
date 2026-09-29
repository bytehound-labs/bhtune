# Windows installer

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## `pkg-windows-installer`: BHTune plus an optional OPC DA gateway

`installer/windows/bhtune-installer.nsi` is the primary Windows distribution. It always installs
`bhtune.exe`/`bhtune-server.exe` and can install the official 32-bit
`opcda-bridge-gateway` as a companion component. The gateway is not built from source in this
repository: `installer/windows/opcda-gateway-release.json` pins the exact stable upstream tag,
source commit, archive/executable SHA-256 values, `i686-pc-windows-msvc`/`I386` architecture,
release-workflow blob, and supported protocol line. `.github/workflows/windows-installer.yml`
independently verifies the upstream checksum and Sigstore evidence, GitHub provenance, archive
layout, PE architecture, version, and compatibility metadata before embedding the executable,
MIT license, notice, release contract, and generated verification manifest. The installer never
downloads a gateway at runtime.

**Component defaults distinguish interactive and unattended installs.** A clean interactive
install selects and starts the gateway after displaying that it is unauthenticated, runs as
`LocalSystem`, and listens on `0.0.0.0:7600`. A clean silent install cannot display that warning
and therefore remains gateway-free unless `/INSTALL_GATEWAY=1` is supplied; a newly selected
gateway starts unless `/START_GATEWAY=0` is also supplied. Existing schema-2 and gateway-free
schema-3 installations preserve absence unless explicitly opted in. Once the installer owns the
gateway, later upgrades continue to manage it even if `/INSTALL_GATEWAY=0` is supplied, and
preserve its previous running/stopped state rather than honoring `/START_GATEWAY`.
`/START_SERVICE` remains scoped to `BhtuneServer`.

**The two services have separate identities and one transaction.** `BhtuneServer` remains an
automatic `LocalService` service bound to `127.0.0.1:8787`. `OpcdaBridgeGateway` is an automatic
`LocalSystem` service whose executable lives under
`%ProgramFiles%\ByteHound\bhtune\gateway\`; its configuration, persistent search index and
SQLite sidecars, build metadata, and logs live under
`%ProgramData%\ByteHound\bhtune\gateway\`. The installer never creates or modifies Windows
Firewall rules. Before mutation it refuses an unowned SCM registration (including one WMI
cannot inspect), unexpected managed-path content, or an unexpected TCP `7600` listener. A
running managed gateway must have a positive SCM PID, every listener must belong to that PID,
the process path must match the managed executable, and at least one listener must bind
`0.0.0.0:7600`.

**Upgrade and recovery are ownership-gated across both services.** Both services stop before
backup/replacement. The one verified rollback snapshot includes BHTune Program Files and the
complete gateway ProgramData tree; recursive capture rejects reparse points and rechecks each
source before copying. A candidate gateway is hash/version/architecture/configuration checked,
registered, transiently started, listener-validated, and smoke-tested with
`bhtune opc --output json gateway-info --bridge-host 127.0.0.1:7600`. The installer compares
the returned application version and core/namespace/indexed-search protocol ranges with the
embedded release contract. This gateway-wide RPC does not contact OPCEnum or an OPC DA server;
actual server enumeration and read-only tag access remain target-host acceptance checks.
Failure restores payloads, data, service definitions, and prior states. Uninstall removes only
marker-proven service registrations and Program Files payloads and preserves all ProgramData.
The narrow pre-journal window can leave only empty fixed directories after power loss; no
service, payload, or data replacement has occurred at that point, and a later run can safely
reuse them.

**Validation is intentionally split.** The cross-platform contract suite covers release and
payload tamper checks, service/listener ownership, schema-2 compatibility, rollback/recovery,
reparse-point rejection, PowerShell call binding, and large redirected child-process output.
The Windows SYSTEM lifecycle diagnostic covers the silent opt-in boundary, explicit
installation, opt-out/add-on, stopped/running upgrades, injected two-service rollback,
ProgramData-preserving uninstall, unowned service/listener refusal, and an unchanged firewall
fingerprint. Its one-hour overall budget covers the complete real-SCM matrix while each
installer or uninstaller process remains capped at two minutes.

## Acceptance and release status

The base two-binary installer source, reusable dry-run workflow, clean-provenance build, and
lifecycle acceptance are complete. Windows 11 and Windows Server 2016 both verified `BhtuneServer`
registration, health/version validation, rollback, uninstall preservation, and the expected isolated
BHTune listener. The installer-managed gateway extension embeds a pinned, independently verified
official 32-bit gateway as an optional component and adds ownership-gated two-service
install/upgrade/rollback/uninstall behavior. The earlier delayed-uninstall interruption was not
reproduced after SCM/WMI hardening, so its original trace has no definitive root-cause diagnosis.
Stable-release attachment remains deferred until the coordinated release-workflow change.
