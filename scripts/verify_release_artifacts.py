#!/usr/bin/env python3
"""Verify the published artifacts and evidence for a BHTune release canary."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path


CANARY_TAG = re.compile(r"^v\d+\.\d+\.\d+-rc\.\d+$")
ARCHIVE_TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
)


class ArtifactVerificationError(ValueError):
    """Raised when a release asset or its evidence is invalid."""


def validate_canary_tag(tag: str) -> str:
    """Return *tag* when it is a stable SemVer prerelease tag."""
    if not CANARY_TAG.fullmatch(tag):
        raise ArtifactVerificationError(
            f"release canary tag must match vMAJOR.MINOR.PATCH-rc.N: {tag!r}"
        )
    return tag


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _checksum_entries(path: Path) -> dict[str, str]:
    entries: dict[str, str] = {}
    for line_number, raw_line in enumerate(
        path.read_text(encoding="utf-8").splitlines(), start=1
    ):
        line = raw_line.strip()
        if not line:
            continue
        parts = line.split(maxsplit=1)
        if len(parts) != 2:
            raise ArtifactVerificationError(
                f"malformed checksum line {line_number} in {path.name}"
            )
        digest, name = parts
        if not re.fullmatch(r"[0-9a-fA-F]{64}", digest):
            raise ArtifactVerificationError(
                f"invalid SHA-256 digest on line {line_number} in {path.name}"
            )
        name = name.lstrip("*")
        entries[Path(name).name] = digest.lower()
    if not entries:
        raise ArtifactVerificationError(f"checksum file is empty: {path.name}")
    return entries


def _require_json_object(path: Path, description: str) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ArtifactVerificationError(
            f"{description} is not valid JSON: {path.name}"
        ) from error
    if not isinstance(value, dict):
        raise ArtifactVerificationError(f"{description} must be a JSON object")
    return value


def _verify_manifest_entry(
    manifest: dict[str, str], asset: Path, *, manifest_name: str
) -> None:
    expected = manifest.get(asset.name)
    if expected is None:
        raise ArtifactVerificationError(
            f"{manifest_name} has no entry for {asset.name}"
        )
    actual = _sha256(asset)
    if actual != expected:
        raise ArtifactVerificationError(
            f"{asset.name} does not match {manifest_name}: "
            f"expected {expected}, got {actual}"
        )


def _archive_paths(assets_dir: Path, tag: str) -> dict[str, Path]:
    return {
        target: assets_dir / f"bhtune-{tag}-{target}{'.zip' if target.endswith('msvc') else '.tar.gz'}"
        for target in ARCHIVE_TARGETS
    }


def _require_archives(archives: dict[str, Path]) -> None:
    missing_archives = [path.name for path in archives.values() if not path.is_file()]
    if missing_archives:
        raise ArtifactVerificationError(
            "missing platform archive(s): " + ", ".join(missing_archives)
        )


def _require_packages(assets_dir: Path) -> list[Path]:
    debs = sorted(assets_dir.glob("*.deb"))
    rpms = sorted(assets_dir.glob("*.rpm"))
    if len(debs) != 1:
        raise ArtifactVerificationError(
            f"expected exactly one .deb package, found {len(debs)}"
        )
    if len(rpms) != 1:
        raise ArtifactVerificationError(
            f"expected exactly one .rpm package, found {len(rpms)}"
        )
    return [debs[0], rpms[0]]


def _verify_evidence(assets_dir: Path, product_assets: list[Path]) -> tuple[Path, Path, Path]:
    manifest_path = assets_dir / "release-assets.sha256"
    if not manifest_path.is_file():
        raise ArtifactVerificationError("missing release-assets.sha256")
    manifest = _checksum_entries(manifest_path)

    sbom_path = assets_dir / "release-sbom.cdx.json"
    if not sbom_path.is_file():
        raise ArtifactVerificationError("missing release-sbom.cdx.json")
    sbom = _require_json_object(sbom_path, "release SBOM")
    if sbom.get("bomFormat") != "CycloneDX":
        raise ArtifactVerificationError("release SBOM is not a CycloneDX document")
    if not isinstance(sbom.get("components"), list):
        raise ArtifactVerificationError("release SBOM has no components array")

    provenance_path = assets_dir / "release-provenance.bundle.json"
    if not provenance_path.is_file():
        raise ArtifactVerificationError("missing release-provenance.bundle.json")
    _require_json_object(provenance_path, "release provenance bundle")

    for asset in product_assets:
        _verify_manifest_entry(manifest, asset, manifest_name=manifest_path.name)
        signature_path = assets_dir / f"{asset.name}.sigstore.json"
        if not signature_path.is_file():
            raise ArtifactVerificationError(f"missing Sigstore bundle for {asset.name}")
        _require_json_object(signature_path, f"Sigstore bundle for {asset.name}")
    return manifest_path, sbom_path, provenance_path


def _verify_archive_checksums(assets_dir: Path, archives: dict[str, Path]) -> None:
    for archive in archives.values():
        suffix = ".zip" if archive.name.endswith(".zip") else ".tar.gz"
        checksum_path = assets_dir / f"{archive.name[:-len(suffix)]}.sha256"
        if not checksum_path.is_file():
            raise ArtifactVerificationError(
                f"missing published checksum file for {archive.name}"
            )
        archive_manifest = _checksum_entries(checksum_path)
        _verify_manifest_entry(archive_manifest, archive, manifest_name=checksum_path.name)


def verify_release_assets(assets_dir: Path, tag: str) -> dict:
    """Verify product assets, checksums, SBOM, provenance, and signatures."""
    tag = validate_canary_tag(tag)
    if not assets_dir.is_dir():
        raise ArtifactVerificationError(f"asset directory does not exist: {assets_dir}")
    archives = _archive_paths(assets_dir, tag)
    _require_archives(archives)
    packages = _require_packages(assets_dir)
    product_assets = [*archives.values(), *packages]
    manifest_path, sbom_path, provenance_path = _verify_evidence(
        assets_dir, product_assets
    )
    _verify_archive_checksums(assets_dir, archives)

    return {
        "tag": tag,
        "product_assets": [asset.name for asset in product_assets],
        "verified_sha256_manifest": manifest_path.name,
        "verified_sbom": sbom_path.name,
        "verified_provenance": provenance_path.name,
        "verified_sigstore_bundles": [
            f"{asset.name}.sigstore.json" for asset in product_assets
        ],
    }


def _safe_report_path(path: Path) -> Path:
    if "\x00" in str(path) or ".." in path.parts:
        raise ArtifactVerificationError(f"unsafe report path: {path}")
    resolved = path.expanduser().resolve()
    if not resolved.parent.is_dir():
        raise ArtifactVerificationError(f"report directory does not exist: {resolved.parent}")
    return resolved


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets-dir", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args(argv)
    try:
        report = verify_release_assets(args.assets_dir, args.tag)
    except ArtifactVerificationError as error:
        print(f"release artifact verification failed: {error}", file=sys.stderr)
        return 1

    rendered = json.dumps(report, indent=2, sort_keys=True)
    print(rendered)
    if args.report:
        try:
            _safe_report_path(args.report).write_text(rendered + "\n", encoding="utf-8")
        except (ArtifactVerificationError, OSError) as error:
            print(f"release artifact verification failed: {error}", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
