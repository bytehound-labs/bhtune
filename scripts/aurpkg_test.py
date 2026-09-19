#!/usr/bin/env python3
"""Focused, network-free tests for scripts/aurpkg."""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
GENERATOR = ROOT / "scripts" / "aurpkg"
UNIT_PATH = Path("packaging/systemd/bhtune-server.service")
PRERELEASE_TAG = "v2.4.6-rc.1"
REQUIRES_MAKEPKG = unittest.skipUnless(
    shutil.which("makepkg"),
    "makepkg is unavailable; metadata rendering tests are skipped",
)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class AurPkgTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        if hasattr(os, "geteuid") and os.geteuid() == 0:
            raise unittest.SkipTest("scripts/aurpkg rejects root execution")
        cls.work_root = Path(tempfile.mkdtemp(prefix=".aurpkg-test-work-", dir=ROOT))

    @classmethod
    def tearDownClass(cls) -> None:
        shutil.rmtree(cls.work_root, ignore_errors=True)

    def setUp(self) -> None:
        self.case_dir = Path(tempfile.mkdtemp(prefix="case-", dir=self.work_root))
        self.archive = self.case_dir / "bhtune-fixture.tar.gz"
        self.archive.write_bytes(b"controlled bhtune release archive\n")

    def tearDown(self) -> None:
        shutil.rmtree(self.case_dir, ignore_errors=True)

    @property
    def inventory(self) -> list[Path]:
        man_pages = sorted(Path("man").glob("*.1"))
        return [
            Path("LICENSE"),
            Path("README.md"),
            *man_pages,
            Path("completions/bhtune.bash"),
            Path("completions/_bhtune"),
            Path("completions/bhtune.fish"),
            UNIT_PATH,
        ]

    def manifest(self) -> Path:
        manifest = self.case_dir / "checksums.sha256"
        lines = [
            f"{sha256(ROOT / path)}  {path.as_posix()}" for path in self.inventory
        ]
        manifest.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return manifest

    def run_generator(
        self,
        tag: str = "v1.2.3",
        *,
        dry_run: bool = True,
        extra: list[str] | None = None,
        runtime_deps: tuple[str, ...] | None = ("glibc",),
    ) -> subprocess.CompletedProcess[str]:
        output = self.case_dir / "output"
        args = [
            str(GENERATOR),
            "--tag",
            tag,
            "--archive-path",
            str(self.archive),
            "--output-dir",
            str(output),
        ]
        if runtime_deps is not None:
            for dependency in runtime_deps:
                args.extend(["--runtime-dep", dependency])
        if dry_run:
            args.append("--dry-run")
        if extra:
            args.extend(extra)
        return subprocess.run(
            args,
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )

    def run_publication(self, *, tag: str = "v1.2.3", extra: list[str] | None = None):
        args = [
            "--archive-sha256",
            sha256(self.archive),
            "--checksums-file",
            str(self.manifest()),
        ]
        if extra:
            args.extend(extra)
        return self.run_generator(tag=tag, dry_run=False, extra=args)

    def assert_success(self, result: subprocess.CompletedProcess[str]) -> None:
        self.assertEqual(
            result.returncode,
            0,
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )

    def assert_failure(
        self, result: subprocess.CompletedProcess[str], message: str
    ) -> None:
        self.assertNotEqual(
            result.returncode,
            0,
            f"unexpected success:\n{result.stdout}\n{result.stderr}",
        )
        self.assertIn(message, result.stderr)

    def generated_files(self) -> tuple[Path, Path]:
        output = self.case_dir / "output"
        return output / "PKGBUILD", output / ".SRCINFO"

    @REQUIRES_MAKEPKG
    def test_publication_accepts_stable_tag_and_generates_srcinfo(self) -> None:
        result = self.run_publication()
        self.assert_success(result)

        pkgbuild, srcinfo = self.generated_files()
        self.assertTrue(pkgbuild.is_file())
        self.assertTrue(srcinfo.is_file())
        self.assertIn("pkgbase = bhtune-bin", srcinfo.read_text(encoding="utf-8"))
        self.assertIn("pkgver = 1.2.3", srcinfo.read_text(encoding="utf-8"))

    def test_publication_rejects_prerelease_tag(self) -> None:
        result = self.run_publication(tag="v1.2.3-rc.1")
        self.assert_failure(result, "publication mode requires an exact stable tag")

    @REQUIRES_MAKEPKG
    def test_dry_run_uses_controlled_fixture_without_network_or_explicit_hashes(
        self,
    ) -> None:
        result = self.run_generator(tag="v2.4.6")
        self.assert_success(result)
        pkgbuild, srcinfo = self.generated_files()
        self.assertTrue(pkgbuild.is_file())
        self.assertTrue(srcinfo.is_file())

    @REQUIRES_MAKEPKG
    def test_dry_run_accepts_prerelease_with_arch_safe_pkgver(self) -> None:
        result = self.run_generator(tag=PRERELEASE_TAG)
        self.assert_success(result)
        pkgbuild, _ = self.generated_files()
        self.assertIn("pkgver=2.4.6.rc.1", pkgbuild.read_text(encoding="utf-8"))

    @REQUIRES_MAKEPKG
    def test_prerelease_explicit_pkgver_must_use_arch_converted_value(self) -> None:
        result = self.run_generator(
            tag=PRERELEASE_TAG,
            extra=["--pkgver", "2.4.6.rc.1"],
        )
        self.assert_success(result)

        result = self.run_generator(
            tag=PRERELEASE_TAG,
            extra=["--pkgver", "2.4.6_rc.1"],
        )
        self.assert_failure(result, "expected 2.4.6.rc.1")

        result = self.run_generator(
            tag=PRERELEASE_TAG,
            extra=["--pkgver", PRERELEASE_TAG],
        )
        self.assert_failure(result, "package version contains unsafe characters")

    def test_package_version_must_match_tag(self) -> None:
        result = self.run_generator(
            extra=["--pkgver", "1.2.4"],
        )
        self.assert_failure(result, "does not match tag v1.2.3")

    def test_mutable_archive_url_is_rejected(self) -> None:
        result = self.run_generator(
            extra=[
                "--archive-url",
                "https://github.com/bytehound-labs/bhtune/archive/refs/heads/main.tar.gz",
            ]
        )
        self.assert_failure(result, "mutable branch URLs are not allowed")

    def test_archive_sha256_is_validated_against_fixture(self) -> None:
        result = self.run_generator(
            extra=["--archive-sha256", "0" * 64],
        )
        self.assert_failure(result, "SHA-256 mismatch for archive fixture")

        result = self.run_generator(extra=["--archive-sha256", "not-a-hash"])
        self.assert_failure(result, "archive SHA-256 must be a 64-character")

    @REQUIRES_MAKEPKG
    def test_runtime_dependency_manifest_is_sorted_and_deduplicated(self) -> None:
        dependency_file = self.case_dir / "runtime-deps.txt"
        dependency_file.write_text(
            "# Resolved from ldd and pacman -Qo\nzlib\nglibc\nzlib\n",
            encoding="utf-8",
        )
        result = self.run_generator(
            runtime_deps=(),
            extra=["--runtime-deps-file", str(dependency_file)],
        )
        self.assert_success(result)
        pkgbuild, _ = self.generated_files()
        content = pkgbuild.read_text(encoding="utf-8")
        self.assertIn("depends=('glibc' 'zlib')", content)

    def test_runtime_dependency_names_are_validated(self) -> None:
        result = self.run_generator(
            runtime_deps=(),
            extra=["--runtime-dep", "glibc;touch"],
        )
        self.assert_failure(result, "unsafe runtime dependency name")

    @REQUIRES_MAKEPKG
    def test_runtime_dependencies_are_derived_from_built_archive_with_ldd(self) -> None:
        if not shutil.which("ldd") or not shutil.which("pacman"):
            self.skipTest("ldd and pacman are required for dependency derivation")

        payload = self.case_dir / "payload"
        payload.mkdir()
        shutil.copy2("/usr/bin/true", payload / "bhtune")
        shutil.copy2("/usr/bin/true", payload / "bhtune-server")
        self.archive.unlink()
        subprocess.run(
            ["tar", "-C", str(payload), "-czf", str(self.archive), "."],
            check=True,
        )

        result = self.run_generator(runtime_deps=None)
        self.assert_success(result)
        pkgbuild, _ = self.generated_files()
        content = pkgbuild.read_text(encoding="utf-8")
        self.assertIn("depends=('glibc')", content)

    def test_checksum_manifest_rejects_bad_hashes_and_paths(self) -> None:
        cases = {
            "../LICENSE": "unsafe path traversal",
            "/absolute/path": "unsafe absolute path",
            "man/./bhtune.1": "current-directory component",
            "unexpected.txt": "unexpected ancillary path",
        }
        for path, message in cases.items():
            with self.subTest(path=path):
                manifest = self.case_dir / "bad.sha256"
                manifest.write_text(
                    f"{'0' * 64}  {path}\n",
                    encoding="utf-8",
                )
                result = self.run_generator(
                    extra=["--checksums-file", str(manifest)],
                )
                self.assert_failure(result, message)

        manifest = self.case_dir / "short.sha256"
        manifest.write_text(f"{'0' * 63}  LICENSE\n", encoding="utf-8")
        result = self.run_generator(extra=["--checksums-file", str(manifest)])
        self.assert_failure(result, "invalid checksum manifest line")

        manifest = self.case_dir / "duplicate.sha256"
        manifest.write_text(
            f"{'0' * 64}  LICENSE\n{'1' * 64}  LICENSE\n",
            encoding="utf-8",
        )
        result = self.run_generator(extra=["--checksums-file", str(manifest)])
        self.assert_failure(result, "duplicate checksum")

    def test_explicit_checksums_must_match_current_inventory(self) -> None:
        manifest = self.manifest()
        with manifest.open("r+b") as manifest_file:
            manifest_file.write(b"0" * 64)
        result = self.run_generator(
            dry_run=False,
            extra=[
                "--archive-sha256",
                sha256(self.archive),
                "--checksums-file",
                str(manifest),
            ],
        )
        self.assert_failure(result, "SHA-256 mismatch for LICENSE")

    @REQUIRES_MAKEPKG
    def test_generated_pkgbuild_contains_current_inventory_and_install_rules(
        self,
    ) -> None:
        result = self.run_generator()
        self.assert_success(result)
        pkgbuild, _ = self.generated_files()
        content = pkgbuild.read_text(encoding="utf-8")

        self.assertIn("# Maintainer: Mike Boiko <mike@bytehound.ca>", content)
        self.assertIn("pkgname=bhtune-bin", content)
        self.assertIn("pkgver=1.2.3", content)
        self.assertIn("arch=('x86_64')", content)
        self.assertIn("license=('AGPL-3.0-or-later')", content)
        self.assertIn("provides=('bhtune')", content)
        self.assertIn("conflicts=('bhtune')", content)
        self.assertIn("depends=('glibc')", content)
        self.assertIn("options=('!debug')", content)
        self.assertIn("install -Dm755 bhtune ", content)
        self.assertIn("install -Dm755 bhtune-server ", content)
        self.assertIn(
            "usr/lib/systemd/system/bhtune-server.service",
            content,
        )

        for path in self.inventory:
            self.assertIn(
                f"https://github.com/bytehound-labs/bhtune/raw/v1.2.3/{path.as_posix()}",
                content,
            )
        for man_page in sorted(Path("man").glob("*.1")):
            self.assertIn(
                f'install -Dm644 "man-{man_page.name}"',
                content,
            )

        self.assertIn(
            "usr/share/bash-completion/completions/bhtune",
            content,
        )
        self.assertIn("usr/share/zsh/site-functions/_bhtune", content)
        self.assertIn(
            "usr/share/fish/vendor_completions.d/bhtune.fish",
            content,
        )

    @REQUIRES_MAKEPKG
    def test_systemd_source_uses_canonical_unit_path(self) -> None:
        result = self.run_generator()
        self.assert_success(result)
        pkgbuild, _ = self.generated_files()
        content = pkgbuild.read_text(encoding="utf-8")

        canonical_source = (
            "https://github.com/bytehound-labs/bhtune/raw/"
            f"v1.2.3/{UNIT_PATH.as_posix()}"
        )
        legacy_unit_path = Path("packaging/systemd") / "deb" / UNIT_PATH.name
        self.assertIn(canonical_source, content)
        self.assertNotIn(legacy_unit_path.as_posix(), content)
        self.assertIn(
            'install -Dm644 service-bhtune-server.service '
            '"$pkgdir/usr/lib/systemd/system/bhtune-server.service"',
            content,
        )

    @REQUIRES_MAKEPKG
    def test_generated_pkgbuild_does_not_compile_sources(self) -> None:
        result = self.run_generator()
        self.assert_success(result)
        pkgbuild, _ = self.generated_files()
        content = pkgbuild.read_text(encoding="utf-8").lower()
        for command in ("cargo", "rustc", "pnpm", "npm", "vite", "cmake"):
            self.assertNotRegex(content, rf"\b{re.escape(command)}\b")

    @REQUIRES_MAKEPKG
    def test_generated_metadata_is_deterministic(self) -> None:
        first = self.run_generator()
        self.assert_success(first)
        first_pkgbuild, first_srcinfo = self.generated_files()
        first_contents = (
            first_pkgbuild.read_bytes(),
            first_srcinfo.read_bytes(),
        )

        shutil.rmtree(self.case_dir / "output")
        second = self.run_generator()
        self.assert_success(second)
        second_pkgbuild, second_srcinfo = self.generated_files()
        self.assertEqual(
            first_contents,
            (second_pkgbuild.read_bytes(), second_srcinfo.read_bytes()),
        )

    @REQUIRES_MAKEPKG
    def test_source_urls_are_tag_pinned_and_source_hashes_align(self) -> None:
        result = self.run_generator()
        self.assert_success(result)
        pkgbuild, _ = self.generated_files()
        content = pkgbuild.read_text(encoding="utf-8")

        self.assertIn(
            "https://github.com/bytehound-labs/bhtune/releases/download/"
            "v1.2.3/bhtune-v1.2.3-x86_64-unknown-linux-gnu.tar.gz",
            content,
        )
        self.assertNotIn("/raw/main/", content)
        source_marker = "source=(\n"
        checksums_marker = "\n)\nsha256sums=(\n"
        source_start = content.index(source_marker) + len(source_marker)
        checksums_start = content.index(checksums_marker, source_start)
        source_text = content[source_start:checksums_start]
        checksums_start += len(checksums_marker)
        checksums_end = content.index("\n)", checksums_start)
        sources = [
            line.strip()
            for line in source_text.splitlines()
            if line.strip()
        ]
        hashes = [
            line.strip()
            for line in content[checksums_start:checksums_end].splitlines()
            if line.strip()
        ]
        self.assertEqual(len(sources), len(hashes))
        self.assertEqual(len(sources), len(self.inventory) + 1)


if __name__ == "__main__":
    unittest.main()
