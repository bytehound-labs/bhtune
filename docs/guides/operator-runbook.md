---
sidebar_position: 3
---

# Operator runbook

This runbook is for an engineer operating an installed BHTune instance. It covers the
read-only checks and controlled actions around a tune; it does not replace the
[Safety](safety.md) guide or the site-specific procedure for the connected control system.

## 1. Installation acceptance

Before an installation is accepted for operator use, verify all of the following:

- BHTune is installed from a release artifact whose checksum and release evidence have been
  verified with the [release-verification guide](release-verification.md).
- The server listens on `127.0.0.1:8787` unless a deliberate, documented deployment decision
  says otherwise.
- No installer or package step created a firewall rule or changed the OPC DA gateway.
- `GET /api/health` returns `status: "ok"` and the expected product version.
- A simulator tune completes and appears in history.
- The service identity, configuration path, database path, and log path match the installation
  method.
- The operator can stop and start the service and can locate its logs.

The browser health indicator confirms only that the BHTune HTTP service responds. It does not
test the OPC DA gateway, a controller, or a tag.

## 2. Identify the installation

Use the procedure matching the installed distribution. Do not mix service units or paths from
different installation methods.

| Installation             | Binary/service path                                                                       | Configuration                                                   | Data and logs                                      |
| ------------------------ | ----------------------------------------------------------------------------------------- | --------------------------------------------------------------- | -------------------------------------------------- |
| Windows NSIS             | `%ProgramFiles%\ByteHound\bhtune\bhtune-server.exe`; service `BhtuneServer`               | `%ProgramData%\ByteHound\bhtune\bhtune.toml`                    | `%ProgramData%\ByteHound\bhtune\data\` and `logs\` |
| Windows manual archive   | The path chosen by the operator; service registration uses the binary's `install` command | The path passed to `--config`, if any                           | The configured/default user data directory         |
| Arch, Debian/Ubuntu, RPM | `/usr/bin/bhtune-server`; `bhtune-server.service`                                         | `/etc/bhtune/`                                                  | `/var/lib/bhtune/`, including `logs/`              |
| macOS archive            | `/usr/local/bin/bhtune-server` unless the archive was installed elsewhere                 | `/usr/local/etc/bhtune/` for the supplied LaunchDaemon template | `/usr/local/var/bhtune/` and `/usr/local/var/log/` |
| Docker                   | `/usr/local/bin/bhtune-server` inside the container                                       | Environment variables or a mounted config                       | The mounted `/var/lib/bhtune` volume               |

The Windows NSIS service runs as `NT AUTHORITY\LocalService`. The manual Windows
`bhtune-server.exe install` path follows the application's existing service-registration path
and runs as `LocalSystem`; it is a separate operating model. The package-managed Linux unit
uses `DynamicUser=true`; the supplied macOS LaunchDaemon runs as root because no dedicated
launchd service account is provisioned.

## 3. Service control and logs

### Windows NSIS

Run PowerShell as an administrator when changing service state:

```powershell
Get-Service BhtuneServer
sc.exe qc BhtuneServer
sc.exe query BhtuneServer
Invoke-RestMethod http://127.0.0.1:8787/api/health
Stop-Service BhtuneServer
Start-Service BhtuneServer
```

The application log directory is:

```text
C:\ProgramData\ByteHound\bhtune\logs\
```

The installer keeps its transaction and rollback evidence under:

```text
C:\ProgramData\ByteHound\bhtune\installer\
```

Do not delete that directory while investigating an upgrade or rollback.

### Linux systemd

```sh
sudo systemctl status bhtune-server --no-pager
sudo systemctl start bhtune-server
sudo systemctl stop bhtune-server
sudo systemctl restart bhtune-server
sudo journalctl -u bhtune-server -n 100 --no-pager
curl --fail http://127.0.0.1:8787/api/health
```

For the package-managed unit, inspect the effective service definition and identity:

```sh
systemctl cat bhtune-server
systemctl show bhtune-server \
  --property=User,DynamicUser,ExecStart,Environment,StateDirectory,ConfigurationDirectory
```

The application log directory is normally `/var/lib/bhtune/logs/`. The journal is still the
first place to look for service-manager failures that occur before application logging starts.

### macOS LaunchDaemon

```sh
sudo launchctl print system/com.bytehound-labs.bhtune-server
sudo launchctl kickstart -k system/com.bytehound-labs.bhtune-server
sudo launchctl bootout system/com.bytehound-labs.bhtune-server
sudo launchctl bootstrap system \
  /Library/LaunchDaemons/com.bytehound-labs.bhtune-server.plist
curl --fail http://127.0.0.1:8787/api/health
```

Inspect both the application log and launchd's early-startup capture:

```sh
tail -n 100 /usr/local/var/log/bhtune-server.log
tail -n 100 /usr/local/var/log/bhtune-server.error.log
```

### Docker

```sh
docker ps --filter name=bhtune
docker logs --tail 100 bhtune
docker exec bhtune bhtune history list
curl --fail http://127.0.0.1:8787/api/health
docker stop bhtune
docker start bhtune
```

Keep the named or bind-mounted `/var/lib/bhtune` volume. Replacing a container without the
volume creates a new empty database.

## 4. Configuration and localhost access

Open the browser at:

```text
http://127.0.0.1:8787
```

The native server binds to localhost by default. Docker deliberately binds inside the
container to `0.0.0.0:8787` so an explicitly published Docker port is reachable; publishing
that port is the operator's network-exposure decision.

Before changing a configuration file:

1. Export or copy the database and its `-wal`/`-shm` companions while the service is stopped.
2. Preserve a copy of the current configuration.
3. Make one focused change.
4. Start the service and verify `/api/health`.
5. Run the simulator smoke test before touching a live loop.

The browser Config page writes the same `bhtune.toml` used by the CLI and server. It preserves
unrelated TOML content and creates a timestamped sibling backup when replacing an existing
file. Startup-only settings such as the bind address and database path remain deployment
configuration rather than live browser controls.

## 5. Simulator smoke test

The simulator is the safe functional acceptance path. It does not connect to OPC DA or write
PID constants:

```sh
bhtune simulate --output json > /tmp/bhtune-simulate.json
python3 -m json.tool /tmp/bhtune-simulate.json >/dev/null
bhtune history list --limit 1
```

The JSON command must write one parseable value to stdout. A successful run has a completed
outcome and calculated results; write-back is skipped because the simulator has no PID
constant tags. On a server deployment, start the run from `/runs/new`, wait for the live trend
to finish, and confirm the final run detail has `Completed` and `Restore: confirmed`.

If this check fails, stop before using the OPC DA driver. Inspect the service log, the health
response, the configured database path, and the recorded run outcome.

## 6. Read-only OPC checks

These commands discover and read tags without starting a tune or writing a value:

```sh
bhtune opc servers --bridge-host gateway.plant.local:7600
bhtune opc read --server Matrikon.OPC.Simulation.1 \
  --bridge-host gateway.plant.local:7600 \
  Area01.FIC101.PV
```

Use the exact server ProgID and ItemID returned by the gateway. Do not invent namespace
syntax or infer a tag by stripping punctuation. The HTTP server also exposes capability
discovery when a browser/API client needs it:

```sh
curl --fail \
  'http://127.0.0.1:8787/api/opc/capabilities?bridge_host=gateway.plant.local:7600&opc_server=Matrikon.OPC.Simulation.1'
```

For a large namespace, use the paged `bhtune opc browse`/`bhtune opc search` commands or the
browser's tag browser. A root browse page leaves its opaque session and continuation values
available for follow-up requests:

```sh
bhtune opc browse --server Matrikon.OPC.Simulation.1 \
  --bridge-host gateway.plant.local:7600
```

Close an explicit browse session when it is no longer needed:

```sh
bhtune opc close <session-id>
```

Confirm the read value, quality, timestamp behavior, and any transport error before preparing
a live tune. A read-only check does not prove that a relay write or restore will succeed.

## 7. Pre-tune live-loop checklist

Complete this checklist immediately before a live tune:

1. Identify the loop, controller, DCS/PLC server, gateway host, and exact PV ItemID.
2. Confirm the selected template and its tag mappings.
3. Confirm the loop is safe to place in manual and safe to move through the requested relay
   amplitude.
4. Confirm the PV/MV ranges and initial MV are sensible for the present operating state.
5. Confirm the host and gateway are responsive and that no unrelated high-load work is running.
6. Confirm the configured poll interval is comfortably shorter than the expected oscillation
   period.
7. Confirm `[tuning].timeout_secs`, `op_timeout_secs`, and `restore_timeout_secs` are appropriate.
8. Decide whether the run is observation-only or will request a PID write-back.
9. If writing, choose the response level and verify the destination P/I/D tags.
10. Ensure an independent database backup exists before a packaging upgrade or recovery action.

Never treat a successful server health check as permission to tune. It checks the BHTune process,
not plant readiness.

## 8. Start, observe, cancel, and review

### CLI

Start an observation-only live tune:

```sh
bhtune tune \
  --driver opcda \
  --server Matrikon.OPC.Simulation.1 \
  --bridge-host gateway.plant.local:7600 \
  --tagname FIC101 \
  --template "Yokogawa CentumVP" \
  --process-type flow \
  --controller-type pi \
  --relay-amp 5 \
  --output json > run.json
```

Keep the terminal attached to the process. Press Ctrl+C once to abort and restore; a second
Ctrl+C during restore abandons the restore and requires a manual loop check. Review the result:

```sh
bhtune history show <run-id>
bhtune export <run-id> --format csv > run-<run-id>-samples.csv
```

### Web GUI

Use `/runs/new`, review the form, and select **Start tune**. The run detail page shows the live
PV/MV trend, switch progress, restore result, calculated constants, timing diagnostics, and
write history. **Cancel** uses the same abort-and-restore path as Ctrl+C.

Do not navigate away as a substitute for cancellation. Use **Cancel** and wait for a terminal
outcome before deciding whether the loop needs manual attention.

## 9. Review and apply PID constants

Review the trend, sampling diagnostics, calculated-result validity, and the template-specific
units before applying a value. `marginal` sampling adequacy is advisory, but it requires
engineering review rather than automatic rejection or automatic acceptance.

For a CLI write-back:

```sh
bhtune tune ... --write-pid moderate --yes
```

For a completed web run, use **Review & write** on the chosen response-level row. The modal
shows the exact P/I/D destinations and values. Confirm only after checking the loop identity and
the selected response level.

BHTune pre-reads the current constants, writes and verifies P/I/D one at a time, and rolls back
only the constants confirmed before a failure. A failed rollback is recorded and points to:

```sh
bhtune history revert <run-id> --yes
```

The web **Restore previous values** action targets the newest successful write. Treat a
transport failure, failed readback, or incomplete rollback as an incident, not as a successful
tune.

## 10. Backup, restore, retention, and export

Before copying a live SQLite database, stop the service:

```sh
sudo systemctl stop bhtune-server
cp -a /var/lib/bhtune/bhtune.db /safe/location/
cp -a /var/lib/bhtune/bhtune.db-wal /safe/location/ 2>/dev/null || true
cp -a /var/lib/bhtune/bhtune.db-shm /safe/location/ 2>/dev/null || true
sudo systemctl start bhtune-server
```

Use the equivalent service-control procedure on Windows, macOS, or Docker. Keep the database,
`-wal`, and `-shm` files together. Do not restore a raw copy over a running database.

For a portable history export:

```sh
bhtune export <run-id> --format json --output run-<run-id>.json
bhtune history list --output json > history.json
```

Retention is off by default. A configured positive `retention_days` is swept at startup and by
the server's periodic maintenance task; an operator can preview and run a one-off deletion:

```sh
bhtune history prune --older-than-days 30 --dry-run
bhtune history prune --older-than-days 30
```

Confirm the count and cutoff before the non-dry-run command. Retention deletion cascades the
run's samples, results, and write audit rows.

## 11. Upgrade, rollback, and removal

Before any upgrade, capture the current version, service state, configuration, database
backup, and health response. Keep one known-good package or archive available.

- **Windows NSIS:** the installer validates ownership, preserves the previous service state,
  keeps one verified rollback backup, transiently validates the candidate even when the old
  service was stopped, and restores the prior state after success. An external database is
  outside the automatic backup boundary and requires `/CUSTOM_DB_BACKUP_CONFIRMED=1` only
  after an independent backup.
- **Arch/Debian/RPM:** package upgrades preserve `/etc/bhtune` and `/var/lib/bhtune`; the
  package-managed unit uses `/usr/bin/bhtune-server`. A running service may be restarted by
  package lifecycle scripts, while a stopped service remains stopped.
- **Manual archives:** replace binaries only while the service is stopped, keep the prior
  archive, and validate the health endpoint before restoring the prior service state.
- **Docker:** pull the candidate image, stop the old container, recreate it with the same
  `/var/lib/bhtune` volume and environment, then validate health and the simulator. Do not
  remove the volume during an image upgrade.

If health validation or rollback validation fails, leave the service stopped, preserve the
installer/package logs and database backup, and escalate with the exact version, service
definition, API response, and log output. Do not retry repeatedly against a live loop.

Normal removal deletes package or installer payloads but preserves operator data. Windows
uninstall preserves the complete ProgramData tree. Linux package removal preserves `/etc/bhtune`,
`/var/lib/bhtune`, SQLite sidecars, and logs. Delete retained data only as a separate,
operator-approved archival or decommissioning task.

## 12. Incident response

### Service will not start

1. Do not change the database while the service may still be running.
2. Capture `systemctl status`/`sc query`/`launchctl print`/`docker logs`.
3. Check the configured bind, database path, log directory, and service identity.
4. Confirm the configured database parent and SQLite sidecars are accessible to the service
   account.
5. Run the simulator only after the service is healthy again.

### Tune ended with an incomplete restore

Treat the loop as requiring manual inspection. Record the run ID, restore detail, MV tag,
last confirmed value, and service/gateway logs. Do not start another tune on the same loop until
the operator confirms the mode, setpoint, MV, and mode attribute at the DCS/PLC.

### MV actuation verification failed

Do not assume an accepted OPC write reached the plant. Confirm the actual MV at the DCS/PLC,
review the actuation audit in `bhtune history show <run-id>`, and keep the loop under manual
operator control until the final state is known.

### Database or upgrade recovery failed

Stop the service, preserve the original database and sidecars, preserve the failed installer or
package logs, and use an independently verified backup. The Windows installer cannot roll back
an external database, and package/manual archive installs do not provide an automatic database
rollback.

## Related procedures

- [Installation](../getting-started/installation.md)
- [Safety](safety.md)
- [Release verification](release-verification.md)
- [Release automation](releasing.md)
