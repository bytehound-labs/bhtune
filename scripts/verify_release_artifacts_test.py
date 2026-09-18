import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from verify_release_artifacts import (
    ArtifactVerificationError,
    verify_release_assets,
)


class ReleaseArtifactTests(unittest.TestCase):
    def make_release(self):
        directory = Path(tempfile.mkdtemp())
        tag = "v0.1.0-rc.1"
        archives = [
            directory / f"bhtune-{tag}-x86_64-unknown-linux-gnu.tar.gz",
            directory / f"bhtune-{tag}-aarch64-apple-darwin.tar.gz",
            directory / f"bhtune-{tag}-x86_64-pc-windows-msvc.zip",
        ]
        packages = [directory / "bhtune_0.1.0~rc.1-1_amd64.deb", directory / "bhtune-0.1.0-1.x86_64.rpm"]
        product_assets = [*archives, *packages]
        for index, asset in enumerate(product_assets):
            asset.write_bytes(f"asset-{index}".encode())
            digest = hashlib.sha256(asset.read_bytes()).hexdigest()
            (directory / f"{asset.name}.sigstore.json").write_text(
                json.dumps({"bundle": asset.name}),
                encoding="utf-8",
            )
            if asset in archives:
                (directory / f"{asset.name}.sha256").write_text(
                    f"{digest}  {asset.name}\n",
                    encoding="utf-8",
                )

        manifest = "\n".join(
            f"{hashlib.sha256(asset.read_bytes()).hexdigest()}  {asset.name}"
            for asset in product_assets
        )
        (directory / "release-assets.sha256").write_text(manifest + "\n", encoding="utf-8")
        (directory / "release-sbom.cdx.json").write_text(
            json.dumps({"bomFormat": "CycloneDX", "components": []}),
            encoding="utf-8",
        )
        (directory / "release-provenance.bundle.json").write_text(
            json.dumps({"dsseEnvelope": {}}),
            encoding="utf-8",
        )
        return directory, tag

    def test_validates_all_platform_assets_and_evidence(self):
        directory, tag = self.make_release()
        report = verify_release_assets(directory, tag)
        self.assertEqual(report["tag"], tag)
        self.assertEqual(len(report["product_assets"]), 5)

    def test_rejects_stable_tag(self):
        directory, _ = self.make_release()
        with self.assertRaises(ArtifactVerificationError):
            verify_release_assets(directory, "v0.1.0")

    def test_rejects_checksum_mismatch(self):
        directory, tag = self.make_release()
        manifest = directory / "release-assets.sha256"
        lines = manifest.read_text(encoding="utf-8").splitlines()
        digest, name = lines[0].split(maxsplit=1)
        replacement = ("0" if digest[0] != "0" else "1") + digest[1:]
        lines[0] = f"{replacement}  {name}"
        manifest.write_text("\n".join(lines) + "\n", encoding="utf-8")
        with self.assertRaises(ArtifactVerificationError):
            verify_release_assets(directory, tag)

    def test_rejects_missing_signature(self):
        directory, tag = self.make_release()
        next(directory.glob("*.sigstore.json")).unlink()
        with self.assertRaises(ArtifactVerificationError):
            verify_release_assets(directory, tag)
