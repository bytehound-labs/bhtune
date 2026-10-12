---
sidebar_position: 4
---

# Release verification

Use this procedure to verify a BHTune release artifact before installation or release-candidate
validation. Stable releases are the supported installation channel; release candidates are for
validation only.

The release workflow produces Linux, macOS, and Windows archives, `release-assets.sha256`,
`release-sbom.cdx.json`,
`release-provenance.bundle.json`, and a `.sigstore.json` bundle for every release asset.
The Windows NSIS installer is an additional workflow artifact produced from the exact Windows
archive by its reusable workflow. Until the release-automation handoff attaches it to the
GitHub Release, retrieve it from the installer workflow's artifact rather than from
`gh release download`. It is not Authenticode-signed.

## Evidence and what it proves

| Evidence                                 | Proves                                                                                                   | Does not prove                                                                            |
| ---------------------------------------- | -------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| SHA-256 manifest or per-archive checksum | The downloaded bytes match the published digest                                                          | Who built the file or whether the source was the intended commit                          |
| Sigstore bundle                          | Keyless signature and the expected certificate/workflow identity, when verified with the expected policy | Authenticode publisher trust or absence of vulnerabilities                                |
| GitHub artifact provenance               | The attested subject checksum was produced by the expected GitHub Actions build provenance               | That every dependency is safe or that the downloaded file was not replaced after download |
| CycloneDX SBOM                           | The dependency/component inventory emitted for the release asset set                                     | A security guarantee or a signature over each individual binary                           |
| Version/health/simulator checks          | The extracted executable runs and reports the expected application version                               | That a live OPC loop is safe to tune                                                      |

Run the integrity checks before installing anything. Keep the downloaded evidence and the
terminal output in the release record.

## 1. Select the expected tag and asset set

Stable publication uses an exact tag such as `v0.1.0`. Release candidates use a tag such as
`v0.1.0-rc.1`; they are validation-only and must not be used as supported installations.
Arbitrary refs are dry-run inputs only.

The release matrix contains:

```text
bhtune-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
bhtune-vX.Y.Z-aarch64-apple-darwin.tar.gz
bhtune-vX.Y.Z-x86_64-pc-windows-msvc.zip
```

Per-archive checksum assets omit the archive extension. For example, the checksum for
`bhtune-v0.1.0-rc.1-x86_64-unknown-linux-gnu.tar.gz` is
`bhtune-v0.1.0-rc.1-x86_64-unknown-linux-gnu.sha256`.

The exact package filenames are authoritative; do not rename an asset before verification.
The archive contains both `bhtune` and `bhtune-server`, plus `LICENSE` and `README.md`.

List the release without downloading it:

```sh
gh release view vX.Y.Z --repo bytehound-labs/bhtune
gh release view vX.Y.Z --repo bytehound-labs/bhtune \
  --json tagName,isDraft,isPrerelease,assets
```

Confirm that the tag, prerelease state, and platform assets match the release being approved.

## 2. Download evidence and product assets

Use a clean, dedicated directory:

```sh
tag=vX.Y.Z
mkdir -p "bhtune-$tag"
cd "bhtune-$tag"
gh release download "$tag" \
  --repo bytehound-labs/bhtune \
  --pattern 'bhtune-*' \
  --pattern 'release-assets.sha256' \
  --pattern 'release-sbom.cdx.json' \
  --pattern 'release-provenance.bundle.json' \
  --pattern '*.sigstore.json'
```

Do not use a wildcard download directory that already contains files from another tag. The
checksum manifest is generated from the release directory and therefore covers the release
product assets, not the manifest, SBOM, provenance bundle, or signature bundles themselves.

## 3. Verify SHA-256 integrity

On Linux or macOS, verify the release-wide manifest from the directory containing the assets:

```sh
sha256sum --ignore-missing -c release-assets.sha256
```

On macOS systems without GNU `sha256sum`, use:

```sh
while read -r expected name; do
  actual="$(shasum -a 256 "$name" | awk '{print $1}')"
  test "$actual" = "$expected" || {
    printf 'checksum mismatch: %s\n' "$name" >&2
    exit 1
  }
done < release-assets.sha256
```

On Windows PowerShell:

```powershell
$manifest = Get-Content .\release-assets.sha256
foreach ($line in $manifest) {
  if ([string]::IsNullOrWhiteSpace($line)) { continue }
  $parts = $line -split '\s+', 2
  $expected = $parts[0].ToLowerInvariant()
  $name = $parts[1].TrimStart('*')
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $name).Hash.ToLowerInvariant()
  if ($actual -ne $expected) {
    throw "SHA-256 mismatch for $name"
  }
}
```

If any checksum fails, stop. Do not extract or install that asset, and do not regenerate the
manifest from the downloaded file.

For an individual archive, its published `.sha256` file can also be checked directly:

```sh
sha256sum -c bhtune-vX.Y.Z-x86_64-unknown-linux-gnu.sha256
```

The per-archive checksum filename omits the archive extension.

## 4. Verify Sigstore bundles

Install a trusted Cosign release through the site's approved tool-management process. Verify
each product asset against its adjacent bundle, with the exact workflow identity and OIDC
issuer expected for BHTune:

```sh
cosign verify-blob \
  --bundle bhtune-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz.sigstore.json \
  --certificate-identity \
  'https://github.com/bytehound-labs/bhtune/.github/workflows/release.yml@refs/tags/vX.Y.Z' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  bhtune-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
```

Repeat the command for the macOS archive, Windows archive, and any Windows installer artifact.
The certificate identity must match the workflow and exact tag under review;
do not accept a generic GitHub identity or a signature from an arbitrary branch.

Sigstore verifies keyless signing claims. It does not make an unsigned Windows installer an
Authenticode-trusted publisher artifact.

## 5. Verify GitHub artifact provenance

The release workflow attests `release-assets.sha256`, whose entries cover the product assets.
Verify the published bundle with GitHub CLI:

```sh
gh attestation verify release-assets.sha256 \
  --repo bytehound-labs/bhtune \
  --bundle release-provenance.bundle.json \
  --signer-workflow bytehound-labs/bhtune/.github/workflows/release.yml \
  --source-ref "refs/tags/$tag" \
  --cert-oidc-issuer 'https://token.actions.githubusercontent.com'
```

The command must identify the BHTune repository, the release workflow, the exact tag under
review, and GitHub's Actions OIDC issuer. If the installed GitHub CLI does not support
`--bundle` or `--cert-oidc-issuer`, update it through the approved tooling path or verify the
attestation from the GitHub artifact-attestation record; do not replace provenance verification
with a checksum-only result.

The attestation binds the checksum subject to the workflow provenance. The Sigstore bundle
still matters because it provides the per-file keyless signature and certificate identity.

## 6. Inspect the SBOM

Confirm that the SBOM is a CycloneDX JSON document and contains components:

```sh
python3 - <<'PY'
import json
from pathlib import Path

sbom = json.loads(Path("release-sbom.cdx.json").read_text())
assert sbom["bomFormat"] == "CycloneDX"
assert isinstance(sbom["components"], list)
print(f"SBOM components: {len(sbom['components'])}")
PY
```

Review unexpected components, licenses, and versions against the release change. The SBOM is a
dependency inventory, not a substitute for the repository's `cargo deny`, frontend license,
security, and test gates.

## 7. Verify the Windows installer

The NSIS installer is named:

```text
bhtune-vX.Y.Z-windows-x86_64-installer.exe
```

In the current pre-release workflow, download the installer from the
`bhtune-windows-installer-X.Y.Z` workflow artifact and its evidence from the
`bhtune-windows-installer-evidence-bhtune-vX.Y.Z-windows-x86_64-installer.exe` artifact.
Use `gh run view <run-id>` to confirm the exact artifact names for the run under review. After
the release-automation handoff, the same files may also be attached to the GitHub Release; the
verification commands are unchanged once the installer has been copied into the dedicated
release directory.
Verify its SHA-256 file and Sigstore bundle before execution. The installer-specific checksum
file is authoritative until the installer is included in the release-wide checksum manifest.

```sh
gh run download <run-id> \
  --repo bytehound-labs/bhtune \
  --name bhtune-windows-installer-X.Y.Z \
  --dir .
gh run download <run-id> \
  --repo bytehound-labs/bhtune \
  --name bhtune-windows-installer-evidence-bhtune-vX.Y.Z-windows-x86_64-installer.exe \
  --dir .
```

```powershell
$installer = Get-Item .\bhtune-vX.Y.Z-windows-x86_64-installer.exe
$expected = (Get-Content "$($installer.FullName).sha256" |
  Select-Object -First 1) -split '\s+', 2
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer.FullName).Hash.ToLowerInvariant()
if ($actual -ne $expected[0].ToLowerInvariant()) {
  throw "installer checksum mismatch"
}
```

Verify the installer-specific Sigstore bundle and provenance bundle against the reusable
installer workflow and exact stable tag:

```sh
cosign verify-blob \
  --bundle bhtune-vX.Y.Z-windows-x86_64-installer.exe.sigstore.json \
  --certificate-identity \
  'https://github.com/bytehound-labs/bhtune/.github/workflows/windows-installer.yml@refs/tags/vX.Y.Z' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  bhtune-vX.Y.Z-windows-x86_64-installer.exe

gh attestation verify \
  bhtune-vX.Y.Z-windows-x86_64-installer.exe.sha256 \
  --repo bytehound-labs/bhtune \
  --bundle installer.provenance.bundle.json \
  --signer-workflow bytehound-labs/bhtune/.github/workflows/windows-installer.yml \
  --source-ref 'refs/tags/vX.Y.Z' \
  --cert-oidc-issuer 'https://token.actions.githubusercontent.com'
```

When the installer is attached to the release-wide manifest, verify that entry as well:

```powershell
$expected = (Get-Content .\release-assets.sha256 |
  Where-Object { $_ -match 'bhtune-vX\.Y\.Z-windows-x86_64-installer\.exe$' } |
  ForEach-Object { ($_ -split '\s+', 2)[0].ToLowerInvariant() })
$actual = (Get-FileHash -Algorithm SHA256 `
  .\bhtune-vX.Y.Z-windows-x86_64-installer.exe).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw "installer checksum mismatch" }
```

Then run it only in an isolated Windows acceptance environment and verify the embedded payload
after installation:

```powershell
& .\bhtune-vX.Y.Z-windows-x86_64-installer.exe /S
& "$env:ProgramFiles\ByteHound\bhtune\bhtune.exe" --version
& "$env:ProgramFiles\ByteHound\bhtune\bhtune-server.exe" --version
if (Get-Service OpcdaBridgeGateway -ErrorAction SilentlyContinue) {
  throw "silent clean install unexpectedly enabled the gateway"
}
# Silent installs require an explicit gateway opt-in.
& .\bhtune-vX.Y.Z-windows-x86_64-installer.exe /S /INSTALL_GATEWAY=1
& "$env:ProgramFiles\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe" --version
Invoke-RestMethod http://127.0.0.1:8787/api/health
```

Confirm the health response has `status: "ok"` and version `X.Y.Z`, then inspect:

```powershell
sc.exe qc BhtuneServer
sc.exe qc OpcdaBridgeGateway
Get-Acl "$env:ProgramFiles\ByteHound\bhtune"
Get-Acl "$env:ProgramData\ByteHound\bhtune"
Get-CimInstance Win32_Service -Filter "Name='OpcdaBridgeGateway'"
Get-NetTCPConnection -State Listen -LocalPort 7600
& "$env:ProgramFiles\ByteHound\bhtune\bhtune.exe" opc --output json gateway-info `
  --bridge-host 127.0.0.1:7600
& "$env:ProgramFiles\ByteHound\bhtune\bhtune.exe" opc --output json servers `
  --bridge-host 127.0.0.1:7600
```

The first command verifies the gateway-free silent default; the second invocation explicitly
adds the gateway. The BHTune service must run as `NT AUTHORITY\LocalService`. The gateway must
run as `LocalSystem` from the managed executable, use the managed configuration/log arguments,
register for automatic start, and own every TCP `7600` listener. At least one listener must bind
`0.0.0.0:7600`. `bhtune opc --output json gateway-info` must report the pinned application
version and compatible core, namespace, and indexed-search protocol ranges without requiring
OPCEnum or a registered OPC DA server. `bhtune opc --output json servers` is a separate
target-host integration check; an empty `servers` array is valid on a host with no registered
OPC DA servers.

Verify the embedded upstream release record independently:

```powershell
$gatewayRoot = "$env:ProgramFiles\ByteHound\bhtune\gateway"
$contract = Get-Content "$gatewayRoot\opcda-gateway-release.json" -Raw | ConvertFrom-Json
$provenance = Get-Content "$gatewayRoot\opcda-gateway-provenance.json" -Raw | ConvertFrom-Json
$actualGatewayHash = (Get-FileHash -Algorithm SHA256 `
  "$gatewayRoot\opcda-bridge-gateway.exe").Hash.ToLowerInvariant()
if ($actualGatewayHash -ne ([string]$contract.executable.sha256).ToLowerInvariant()) {
  throw "installed gateway checksum mismatch"
}
if (-not $provenance.sigstore_verified -or
    -not $provenance.github_provenance_verified) {
  throw "embedded gateway verification record is incomplete"
}
```

The checked-in release contract pins the upstream repository, stable tag, source commit,
archive and executable hashes, 32-bit target, release-workflow blob, and required protocol
versions. The installer workflow independently verifies the upstream checksum, Sigstore bundle,
GitHub provenance, archive layout, PE architecture, version, and gateway-wide protocol handshake
before embedding the gateway. Treat any mismatch between the installed files, checked-in contract,
or upstream evidence as a release failure.

The installer keeps BHTune and gateway configuration, databases, indexes, logs, and rollback
state under ProgramData and does not create firewall rules. It is not Authenticode-signed, so
SmartScreen may display an unknown-publisher warning. That warning is separate from the
checksum, Sigstore, and provenance results above.

Repeat lifecycle acceptance with `/INSTALL_GATEWAY=0`,
`/INSTALL_GATEWAY=1 /START_GATEWAY=0`, and managed running and stopped upgrades. Verify that a
gateway-free installation stays gateway-free unless explicitly opted in, a managed gateway
retains its prior state across upgrades, injected candidate failure restores both services and
gateway ProgramData, uninstall preserves the complete ProgramData tree, unowned service and
unexpected listener conflicts fail closed, and the Windows Firewall rule fingerprint does not
change.

Do not run the installer against a production database as a substitute for the documented
upgrade procedure. Its automatic rollback boundary covers only the installer-managed
ProgramData BHTune database plus the managed gateway ProgramData tree; an external BHTune
database requires an independent backup and explicit operator acknowledgement.

## 8. Inspect a platform archive

Each platform archive contains both `bhtune` and `bhtune-server` at its root, plus `LICENSE`
and `README.md`. Inspect its file list after verifying the checksum:

```sh
tar -tzf bhtune-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
unzip -l bhtune-vX.Y.Z-x86_64-pc-windows-msvc.zip
```

Confirm that the archive matches the expected target and contains both executables before
installation.

## 9. Record the verification

Retain a small release record containing:

- the repository, tag, release URL, and commit;
- the exact asset filenames;
- the checksum verification output;
- the Sigstore certificate identity and successful verification output;
- the provenance verification output;
- the SBOM component count and review notes;
- Windows installer service/health evidence, gateway checksum/provenance/listener/read-only
  smoke evidence, and unchanged-firewall evidence, when applicable;

If any evidence is missing, mismatched, or ambiguous, stop the release or installation. Do not
repair a release by editing checksums, replacing a tag, or bypassing the stable-tag and
workflow-identity checks.

## Related procedures

- [Installation](../getting-started/installation.md)
- [Operator runbook](operator-runbook.md)
- [Safety](safety.md)
- [Release automation](releasing.md)
