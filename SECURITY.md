# Security policy

BHTune tunes PID loops on live industrial processes. In its default Full mode it can switch a loop
to Manual, step the manipulated variable during a relay test, and write PID constants to a
controller through an OPC DA gateway. A security defect can therefore have physical consequences,
so vulnerability reports are handled privately.

## Supported versions

BHTune is pre-release software and no stable release exists yet. Security fixes are made only on
the latest `main` branch; they are not backported to release candidates, older commits, or
previously built binaries and container images.

| Version                                                                    | Supported |
| -------------------------------------------------------------------------- | --------- |
| Latest `main`                                                              | Yes       |
| Release candidates (`v0.1.0-rc.*`), older commits, earlier binaries/images | No        |

## Reporting a vulnerability

Do not report vulnerabilities through public GitHub issues, discussions, or pull requests.

Use GitHub private vulnerability reporting instead: open this repository's **Security** tab and
choose **Report a vulnerability**, or go directly to
<https://github.com/bytehound-labs/bhtune/security/advisories/new>. If you cannot use GitHub, email
[info@bytehound.ca](mailto:info@bytehound.ca).

Include as much of the following as you can:

- the affected commit or version (`bhtune --version`, or the `version` field returned by
  `GET /api/health`);
- the server mode and deployment shape (bind address, reverse proxy, configured browser origin);
- the affected component (CLI, HTTP API, web UI, Windows installer, package, or container image);
- reproduction steps or a proof of concept; and
- the impact you expect an attacker could achieve.

Reports are handled on a best-effort basis; there is no guaranteed response or fix timeline.
Please keep the details confidential until a fix is available on `main`.

Test only against installations you own or are explicitly authorized to test, preferably with the
simulator driver. Never test against plant equipment, OPC DA servers, or gateways you are not
authorized to operate.

## Threat model and deployment assumptions

### Full mode (default)

Full mode is the trusted operator surface and has no authentication. Anyone who can reach its HTTP
port can start and cancel tunes (a relay test switches the loop to Manual and steps its manipulated
variable), write or revert PID constants on live control loops through the configured OPC DA
gateway, browse and read OPC tags, control the gateway's search index, and change templates,
configuration, notes, and run history.

- `bhtune-server` binds `127.0.0.1:8787` by default. Binding any other address (`BHTUNE_BIND` or
  the `bind` configuration key) is an explicit opt-in intended only for a trusted, isolated
  network. The Docker image binds `0.0.0.0:8787` inside the container, so publishing its port is
  the equivalent opt-in.
- State-changing requests (every method other than `GET`, `HEAD`, and `OPTIONS`) are protected
  against cross-site request forgery (CSRF). Requests that browser Fetch Metadata marks as
  cross-site are rejected. When an exact origin is pinned with `BHTUNE_ORIGIN` or the `origin`
  configuration key, a request's `Origin` must equal that pin; otherwise the server validates it
  automatically against the request's own `Host` (host and effective port) and browser Fetch
  Metadata. Requests without an `Origin` header are accepted so the CLI and scripts such as `curl`
  keep working. This is CSRF protection, not authentication: it does not identify or authorize
  anyone.
- Never expose Full mode to untrusted networks or the Internet. Authentication, TLS, and audit
  logging are planned remote-access features (see the
  [roadmap](docs/roadmap.md#remote-and-multi-user-access)), not current capabilities.
- The SQLite database, configuration files, and logs are plain, unencrypted files protected only
  by operating-system permissions.

### Demo mode

Demo mode (`BHTUNE_SERVER_MODE=demo`) is a reduced public surface for anonymous simulator
demonstrations, described in the
[public simulator demo guide](docs/guides/public-simulator-demo.md). Its restrictions are enforced
by the server's route tree, not by hiding controls in the browser.

- Only health, capabilities, read-only built-in templates, visitor-scoped simulator runs, and the
  embedded web UI are mounted. OPC discovery, browsing, and reads; PID write and revert;
  configuration and template changes; notes; drafts; and the OpenAPI document and Scalar UI are
  not mounted, so crafted requests cannot reach them.
- Visitors are separated by an anonymous `__Host-bhtune_demo_session` cookie that isolates their
  history and carries their quotas. It is not an account and does not identify a person; the
  server stores only a one-way hash of it. Another visitor's run returns the same `404` as a run
  that does not exist.
- Simulator bounds, request sizes, concurrency, and quotas are fixed application constants that
  configuration cannot weaken.
- Demo mode requires one exact HTTPS browser origin (plain HTTP is accepted only for an explicit
  loopback origin used in local testing) and refuses to start without it. State-changing requests
  must carry exactly that `Origin`.
- Demo mode requires its own dedicated database. Never point it at a Full-mode database, a
  production configuration, or an OPC DA gateway.
- Quota coordination is held in memory, so only one application replica is supported.
- Perimeter defenses remain required: a TLS-terminating reverse proxy, rate limiting or CrowdSec,
  network access controls, and volumetric denial-of-service protection. The server trusts the
  `X-BHTune-Client-IP` header only from the configured `trusted_proxy` peer, and that proxy must
  delete or overwrite any client-supplied value.

### OPC DA gateway

`opcda-bridge-gateway` is a separate, unauthenticated network service that can read and write any
tag its OPC DA server exposes. The Windows installer can optionally install it as a `LocalSystem`
service listening on `0.0.0.0:7600`; the installer never creates firewall rules, so restricting
access to that port is the operator's responsibility. Report vulnerabilities in the gateway itself
privately to
[bytehound-labs/opcda-bridge](https://github.com/bytehound-labs/opcda-bridge/security/advisories/new).

## Scope

Examples of in-scope vulnerabilities:

- bypassing Origin/CSRF validation on a state-changing request;
- escaping Demo mode: reaching Full-mode capabilities (OPC access, PID write or revert,
  configuration or template changes), starting a non-simulator run, or exceeding its fixed
  bounds and quotas;
- reading, streaming, exporting, cancelling, or deleting another Demo visitor's runs, or forging
  or predicting a Demo session token;
- path traversal or arbitrary file reads through the embedded web asset serving;
- injection (SQL, command, or cross-site scripting), including through a malicious template,
  catalog, configuration file, or run request;
- exposure of secrets, credentials, or session tokens;
- missing or weakened security headers, or private responses being cached; and
- tampering with release archives, packages, container images, or the Windows installer, or
  bypassing their signature or provenance verification.

### Known limitations by design

The following are documented properties of the current design, not vulnerabilities on their own:

- Full mode has no authentication, so anyone who can reach a deliberately exposed Full-mode server
  can operate it.
- Full mode accepts state-changing requests that carry no `Origin` header, so the CLI and scripts
  keep working.
- The Full-mode Scalar API reference at `/api/docs` is served without a Content Security Policy
  because it loads its interface from a CDN; Demo mode does not mount it.
- The OPC DA gateway is unauthenticated.
- A Demo session cookie is a bearer token: browsers or profiles that share it share one history and
  one quota until it expires.
- Demo mode supports one application replica and relies on perimeter defenses for
  denial-of-service protection.
- The database, configuration, and logs are plain files protected only by operating-system
  permissions.

Attacks that already require administrator access to the host, write access to BHTune's
configuration, template, or database files, or control of the OPC DA server or gateway are also
out of scope.

## Related documentation

- [Network exposure](docs/guides/safety.md#network-exposure)
- [Public simulator demo](docs/guides/public-simulator-demo.md)
- [Release verification](docs/guides/release-verification.md)
- [Roadmap: remote and multi-user access](docs/roadmap.md#remote-and-multi-user-access)
