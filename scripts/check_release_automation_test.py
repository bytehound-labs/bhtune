#!/usr/bin/env python3

import json
import subprocess
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.error import URLError

from check_release_content import ReleaseContentError, validate_context
from check_release_plz_config import ReleasePlzConfigError, validate_config
from release_policy import conventional_type, is_release_commit, is_release_worthy
from release_rate_limit import ReleaseRateError, count_releases, fetch_releases
from sync_docs_version import DocsVersionError, check, synchronize


class FakeResponse:
    def __init__(self, payload, status=200):
        self.payload = payload
        self.status = status

    def read(self):
        return json.dumps(self.payload).encode()


class GitFixture:
    def __init__(self):
        self.tempdir = tempfile.TemporaryDirectory()
        self.path = Path(self.tempdir.name)
        self.run("git", "init", "-b", "main")
        self.run("git", "config", "user.email", "test@example.com")
        self.run("git", "config", "user.name", "Release Tests")

    def run(self, *args):
        return subprocess.run(
            args,
            cwd=self.path,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def write(self, relative, content):
        path = self.path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    def commit(self, message):
        self.run("git", "add", ".")
        self.run("git", "commit", "-m", message)
        return self.run("git", "rev-parse", "HEAD")

    def close(self):
        self.tempdir.cleanup()


def workspace_files(version="0.1.0"):
    members = ",\n".join(f'  "crates/{name}"' for name in (
        "bhtune-core",
        "bhtune-driver",
        "bhtune-db",
        "bhtune-cli",
        "bhtune-server",
    ))
    files = {
        "Cargo.toml": (
            "[workspace]\n"
            f"members = [\n{members}\n]\n\n"
            "[workspace.package]\n"
            f'version = "{version}"\n'
        )
    }
    for name in (
        "bhtune-core",
        "bhtune-driver",
        "bhtune-db",
        "bhtune-cli",
        "bhtune-server",
    ):
        files[f"crates/{name}/Cargo.toml"] = (
            "[package]\n"
            f'name = "{name}"\n'
            "version.workspace = true\n"
        )
    return files


class ReleasePolicyTests(unittest.TestCase):
    def test_release_commit_and_conventional_commit_policy(self):
        self.assertTrue(is_release_commit("chore(release): prepare v0.1.0"))
        self.assertTrue(is_release_commit("chore: release v1.2.3-rc.1"))
        self.assertFalse(is_release_commit("chore(release): prepare v01.2.3"))
        self.assertEqual(conventional_type("feat(ui)!: add release screen"), "feat")
        self.assertIsNone(conventional_type("not conventional"))
        self.assertTrue(is_release_worthy("feat(core): add deterministic snapshots"))
        self.assertTrue(is_release_worthy("fix!: reject invalid release metadata"))
        self.assertFalse(is_release_worthy("chore(deps): update dependencies"))
        self.assertFalse(is_release_worthy("chore(release): prepare v0.1.0"))
        self.assertFalse(is_release_worthy("feat:"))


class ReleasePlzConfigTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(__file__).resolve().parents[1]

    def test_repository_configuration_is_single_product_and_git_only(self):
        config = validate_config(self.root / "release-plz.toml")
        self.assertFalse(config["workspace"]["publish"])
        self.assertFalse(config["workspace"]["release"])
        self.assertEqual(config["package"][0]["name"], "bhtune-cli")

    def test_non_anchor_package_override_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "release-plz.toml"
            path.write_text(
                (self.root / "release-plz.toml").read_text(encoding="utf-8")
                + '\n[[package]]\nname = "bhtune-core"\nrelease = true\n',
                encoding="utf-8",
            )
            with self.assertRaises(ReleasePlzConfigError):
                validate_config(path)

    def test_registry_token_reference_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "release-plz.toml"
            path.write_text(
                (self.root / "release-plz.toml").read_text(encoding="utf-8")
                + '\nregistry_token = "do-not-use"\n',
                encoding="utf-8",
            )
            with self.assertRaises(ReleasePlzConfigError):
                validate_config(path)


class ReleaseRateLimitTests(unittest.TestCase):
    def test_rate_windows_use_exact_boundary_and_include_all_release_types(self):
        now = datetime(2026, 9, 18, 12, tzinfo=timezone.utc)
        releases = [
            {"published_at": "2026-09-18T11:00:00Z"},
            {"published_at": "2026-09-18T11:00:01Z"},
            {"published_at": "2026-09-18T11:00:02Z"},
            {"created_at": "2026-09-17T12:00:00Z"},
            {"published_at": "2026-09-17T12:00:01Z"},
            {"published_at": "2026-09-17T12:00:02Z"},
            {"published_at": "2026-09-17T12:00:03Z"},
            {"published_at": "2026-09-17T12:00:04Z"},
            {"published_at": "2026-09-17T12:00:05Z"},
            {"published_at": "2026-09-17T12:00:06Z"},
            {"published_at": "2026-09-17T12:00:07Z"},
            {"published_at": "2026-09-17T12:00:08Z"},
        ]
        decision = count_releases(releases, now=now)
        self.assertEqual(decision.hourly_count, 3)
        self.assertEqual(decision.daily_count, 12)
        self.assertFalse(decision.allowed)

    def test_rate_fetch_fails_closed_for_auth_json_and_network_errors(self):
        with self.assertRaises(ReleaseRateError):
            fetch_releases("", "token")
        with self.assertRaises(ReleaseRateError):
            fetch_releases("owner/repo", "token", opener=lambda *_args, **_kwargs: FakeResponse({}, 401))
        with self.assertRaises(ReleaseRateError):
            fetch_releases("owner/repo", "token", opener=lambda *_args, **_kwargs: FakeResponse({}))
        with self.assertRaises(ReleaseRateError):
            fetch_releases(
                "owner/repo",
                "token",
                opener=lambda *_args, **_kwargs: (_ for _ in ()).throw(URLError("offline")),
            )

    def test_rate_fetch_paginates_until_short_page(self):
        calls = []

        def opener(request, **_kwargs):
            calls.append(request.full_url)
            self.assertEqual(request.get_header("Authorization"), "Bearer token")
            if len(calls) == 1:
                return FakeResponse([{"published_at": "2026-09-18T00:00:00Z"}] * 100)
            return FakeResponse([{"published_at": "2026-09-18T00:00:01Z"}])

        releases = fetch_releases("owner/repo", "token", opener=opener)
        self.assertEqual(len(releases), 101)
        self.assertEqual(len(calls), 2)
        self.assertIn("page=2", calls[1])


class ReleaseContentTests(unittest.TestCase):
    def setUp(self):
        self.fixture = GitFixture()
        for path, content in workspace_files().items():
            self.fixture.write(path, content)
        self.fixture.write("README.md", "BHTune\n")
        self.base = self.fixture.commit("chore: initial workspace")

    def tearDown(self):
        self.fixture.close()

    def validate(self, *, base=None, head=None, baseline=None, branch="release-plz-0"):
        return validate_context(
            self.fixture.path,
            base=base or self.base,
            head=head or self.fixture.run("git", "rev-parse", "HEAD"),
            baseline=baseline,
            head_branch=branch,
        )

    def test_first_release_requires_ancestor_baseline_and_accepts_meaningful_content(self):
        self.fixture.write("crates/bhtune-core/src/lib.rs", "pub fn tune() {}\n")
        head = self.fixture.commit("feat(core): add tuning entry point")
        context = self.validate(head=head, baseline=self.base)
        self.assertEqual(context.version, "0.1.0")
        with self.assertRaises(ReleaseContentError):
            self.validate(head=head, baseline=None)
        with self.assertRaises(ReleaseContentError):
            self.validate(head=head, baseline="0" * 40)

    def test_non_release_branch_bypasses_before_release_inputs(self):
        self.assertIsNone(self.validate(branch="feature/ordinary-change"))

    def test_metadata_only_release_is_rejected(self):
        self.fixture.write("CHANGELOG.md", "# Changelog\n")
        head = self.fixture.commit("chore(release): prepare v0.1.0")
        with self.assertRaisesRegex(ReleaseContentError, "only generated"):
            self.validate(head=head, baseline=self.base)

    def test_meaningful_release_areas_are_accepted(self):
        for path in (
            "crates/bhtune-core/src/lib.rs",
            "frontend/src/App.tsx",
            "website/src/custom.ts",
            "docs/guides/release.md",
            ".github/workflows/release.yml",
            "Dockerfile",
            "tests/release_fixture.txt",
        ):
            with self.subTest(path=path):
                self.fixture.write(path, "meaningful\n")
                head = self.fixture.commit(f"feat: change {path}")
                self.assertIsNotNone(self.validate(head=head, baseline=self.base))

    def test_malformed_and_inconsistent_workspace_manifests_are_rejected(self):
        self.fixture.write("Cargo.toml", "[workspace\n")
        malformed = self.fixture.commit("chore: break manifest")
        with self.assertRaises(ReleaseContentError):
            self.validate(head=malformed, baseline=self.base)

        self.fixture.write("Cargo.toml", workspace_files()["Cargo.toml"])
        self.fixture.write(
            "crates/bhtune-core/Cargo.toml",
            '[package]\nname = "bhtune-core"\nversion = "0.2.0"\n',
        )
        inconsistent = self.fixture.commit("chore: inconsistent versions")
        with self.assertRaises(ReleaseContentError):
            self.validate(head=inconsistent, baseline=self.base)

    def test_stable_tag_is_selected_while_prerelease_tag_is_ignored(self):
        self.fixture.write("stable.txt", "stable\n")
        stable = self.fixture.commit("feat: stable baseline content")
        self.fixture.run("git", "tag", "v0.0.9", stable)
        self.fixture.write("rc.txt", "rc\n")
        rc_commit = self.fixture.commit("chore: canary tag point")
        self.fixture.run("git", "tag", "v0.2.0-rc.1", rc_commit)
        self.fixture.write("crates/bhtune-core/src/release.rs", "pub fn release() {}\n")
        head = self.fixture.commit("feat: prepare next stable release")
        context = self.validate(base=rc_commit, head=head)
        self.assertEqual(context.stable_tag, "v0.0.9")
        self.assertEqual(context.comparison, stable)


class DocsVersionTests(unittest.TestCase):
    def setUp(self):
        self.tempdir = tempfile.TemporaryDirectory()
        self.path = Path(self.tempdir.name)
        self.path.joinpath("docs/getting-started").mkdir(parents=True)
        self.path.joinpath("website").mkdir()
        self.path.joinpath("Cargo.toml").write_text(
            '[workspace]\n[workspace.package]\nversion = "0.1.0"\n',
            encoding="utf-8",
        )
        self.path.joinpath("website/sidebars.ts").write_text(
            'export default { docsSidebar: [{ type: "autogenerated", dirName: "." }] };\n',
            encoding="utf-8",
        )
        self.path.joinpath("docs/intro.md").write_text(
            "---\nsidebar_position: 1\n---\n# Intro\n",
            encoding="utf-8",
        )
        self.path.joinpath("docs/getting-started/installation.md").write_text(
            "# Install\n",
            encoding="utf-8",
        )
        self.path.joinpath("docs/getting-started/_category_.json").write_text(
            '{"label": "Getting started"}\n',
            encoding="utf-8",
        )
        self.path.joinpath("docs/internal/v1-checklist.md").parent.mkdir(parents=True)
        self.path.joinpath("docs/internal/v1-checklist.md").write_text("# Internal\n", encoding="utf-8")

    def tearDown(self):
        self.tempdir.cleanup()

    def test_stable_snapshot_is_idempotent_exact_and_excludes_internal_docs(self):
        self.assertFalse(synchronize(self.path, "0.1.0-rc.1"))
        self.assertTrue(synchronize(self.path, "0.1.0"))
        self.assertFalse(synchronize(self.path, "0.1.0"))
        self.assertTrue(check(self.path, "0.1.0"))
        self.assertFalse((self.path / "website/versioned_docs/version-0.1.0/docs/internal").exists())
        versions = json.loads((self.path / "website/versions.json").read_text(encoding="utf-8"))
        self.assertEqual(versions, ["0.1.0"])

    def test_changed_source_refreshes_same_version_and_retains_three(self):
        for version in ("0.1.0", "0.2.0", "0.3.0", "0.4.0"):
            self.assertTrue(synchronize(self.path, version))
        self.assertEqual(
            json.loads((self.path / "website/versions.json").read_text(encoding="utf-8")),
            ["0.4.0", "0.3.0", "0.2.0"],
        )
        self.assertFalse((self.path / "website/versioned_docs/version-0.1.0").exists())
        doc = self.path / "docs/intro.md"
        doc.write_text(doc.read_text(encoding="utf-8") + "\nUpdated.\n", encoding="utf-8")
        self.assertTrue(synchronize(self.path, "0.4.0"))
        self.assertTrue(check(self.path, "0.4.0"))

    def test_tampered_snapshot_fails_check(self):
        synchronize(self.path, "0.1.0")
        snapshot = self.path / "website/versioned_docs/version-0.1.0/intro.md"
        snapshot.write_text("# Tampered\n", encoding="utf-8")
        with self.assertRaises(DocsVersionError):
            check(self.path, "0.1.0")


if __name__ == "__main__":
    unittest.main()
