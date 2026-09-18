"""Validate that a release-plz PR represents a real BHTune product release."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence

WORKSPACE_MEMBERS = (
    "bhtune-core",
    "bhtune-driver",
    "bhtune-db",
    "bhtune-cli",
    "bhtune-server",
)
INITIAL_RELEASE_VERSION = (0, 1, 0)
STABLE_TAG = re.compile(r"^v(?P<version>0|[1-9][0-9]*)\.(?P<minor>0|[1-9][0-9]*)\.(?P<patch>0|[1-9][0-9]*)$")
VERSION_ONLY_LINE = re.compile(
    r"^\s*(?:version\s*=\s*(?:\"[^\"]+\"|\{[^}]*\})|[A-Za-z0-9_-]+\s*=\s*\{[^}]*\bversion\s*=\s*\"[^\"]+\"[^}]*\})\s*$"
)
LOCK_VERSION_LINE = re.compile(r"^\s*version\s*=\s*\"[^\"]+\"\s*$")


class ReleaseContentError(RuntimeError):
    """Raised when release content cannot be proven safe."""


@dataclass(frozen=True)
class ReleaseContext:
    repository: Path
    base: str
    head: str
    comparison: str
    version: str
    stable_tag: str | None


def run_git(repository: Path, *args: str) -> str:
    try:
        result = subprocess.run(
            ["git", "-C", str(repository), *args],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise ReleaseContentError(f"git {' '.join(args)} failed: {detail.strip()}") from error
    return result.stdout


def parse_version(value: object, label: str) -> tuple[int, int, int]:
    if not isinstance(value, str):
        raise ReleaseContentError(f"{label} is not a version string")
    match = re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value)
    if not match:
        raise ReleaseContentError(f"{label} must be a stable X.Y.Z version, got {value!r}")
    return tuple(int(part) for part in match.groups())


def read_manifest(repository: Path, revision: str, relative_path: str) -> dict:
    try:
        content = run_git(repository, "show", f"{revision}:{relative_path}")
        return tomllib.loads(content)
    except tomllib.TOMLDecodeError as error:
        raise ReleaseContentError(f"{relative_path} at {revision} is malformed TOML: {error}") from error


def workspace_versions(repository: Path, revision: str) -> dict[str, str]:
    root = read_manifest(repository, revision, "Cargo.toml")
    package = root.get("workspace", {}).get("package", {})
    root_version = package.get("version")
    version_tuple = parse_version(root_version, f"workspace version at {revision}")
    resolved = {"workspace": ".".join(str(part) for part in version_tuple)}
    members = root.get("workspace", {}).get("members")
    if not isinstance(members, list) or set(members) != {
        f"crates/{member}" for member in WORKSPACE_MEMBERS
    }:
        raise ReleaseContentError(f"workspace members at {revision} do not match the five BHTune crates")

    for member in WORKSPACE_MEMBERS:
        path = f"crates/{member}/Cargo.toml"
        manifest = read_manifest(repository, revision, path)
        package_table = manifest.get("package")
        if not isinstance(package_table, dict):
            raise ReleaseContentError(f"{path} at {revision} has no [package] table")
        member_version = package_table.get("version")
        if isinstance(member_version, dict) and member_version.get("workspace") is True:
            resolved[member] = resolved["workspace"]
        elif isinstance(member_version, str):
            parse_version(member_version, f"{path} version at {revision}")
            resolved[member] = member_version
        else:
            raise ReleaseContentError(f"{path} at {revision} has no resolvable package version")
        if resolved[member] != resolved["workspace"]:
            raise ReleaseContentError(
                f"{path} resolves to {resolved[member]}, not workspace version {resolved['workspace']}"
            )
    return resolved


def version_tuple(value: str) -> tuple[int, int, int]:
    return parse_version(value, "version")


def stable_tags(repository: Path, base: str) -> list[tuple[tuple[int, int, int], str]]:
    tags = run_git(repository, "tag", "--merged", base, "--list", "v*").splitlines()
    result = []
    for tag in tags:
        match = STABLE_TAG.fullmatch(tag.strip())
        if match:
            result.append(
                (
                    (
                        int(match.group("version")),
                        int(match.group("minor")),
                        int(match.group("patch")),
                    ),
                    tag.strip(),
                )
            )
    return sorted(result)


def resolve_comparison(repository: Path, base: str, baseline: str | None) -> tuple[str, str | None]:
    tags = stable_tags(repository, base)
    if tags:
        _, tag = tags[-1]
        comparison = run_git(repository, "rev-list", "-n", "1", tag).strip()
        if not comparison:
            raise ReleaseContentError(f"stable tag {tag} has no commit")
        return comparison, tag
    if not baseline:
        raise ReleaseContentError("RELEASE_BASELINE_SHA is required when no stable product tag exists")
    run_git(repository, "cat-file", "-e", f"{baseline}^{{commit}}")
    try:
        subprocess.run(
            ["git", "-C", str(repository), "merge-base", "--is-ancestor", baseline, base],
            check=True,
            capture_output=True,
            text=True,
        )
    except subprocess.CalledProcessError as error:
        raise ReleaseContentError("RELEASE_BASELINE_SHA is not an ancestor of the PR base") from error
    return baseline, None


def diff_files(repository: Path, comparison: str, head: str) -> list[str]:
    output = run_git(repository, "diff", "--name-only", f"{comparison}..{head}")
    return [line for line in output.splitlines() if line]


def diff_lines(repository: Path, comparison: str, head: str, path: str) -> list[str]:
    output = run_git(repository, "diff", "--unified=0", f"{comparison}..{head}", "--", path)
    return [
        line[1:]
        for line in output.splitlines()
        if line.startswith(("+", "-")) and not line.startswith(("+++", "---"))
    ]


def version_metadata_only(path: str, lines: Iterable[str]) -> bool:
    if path.endswith("Cargo.lock"):
        return all(LOCK_VERSION_LINE.fullmatch(line) for line in lines)
    if path == "Cargo.toml" or path.startswith("crates/") and path.endswith("Cargo.toml"):
        return all(VERSION_ONLY_LINE.fullmatch(line) for line in lines)
    return False


def is_generated_release_metadata(path: str, lines: Sequence[str]) -> bool:
    if path == "CHANGELOG.md" or (
        path.startswith("crates/") and path.endswith("/CHANGELOG.md")
    ):
        return True
    if path.startswith(
        (
            "website/versioned_docs/",
            "website/versioned_sidebars/",
        )
    ):
        return True
    if path == "website/versions.json" or path.endswith("/.release-docs-digest"):
        return True
    return version_metadata_only(path, lines)


def has_meaningful_content(repository: Path, comparison: str, head: str) -> bool:
    paths = diff_files(repository, comparison, head)
    if not paths:
        raise ReleaseContentError("release PR contains no changes relative to its comparison point")
    meaningful = False
    for path in paths:
        lines = diff_lines(repository, comparison, head, path)
        if not is_generated_release_metadata(path, lines):
            meaningful = True
    return meaningful


def validate_context(
    repository: Path,
    *,
    base: str,
    head: str,
    baseline: str | None,
    head_branch: str | None,
) -> ReleaseContext | None:
    if head_branch is not None and not head_branch.startswith("release-plz-"):
        return None
    workspace_versions(repository, base)
    head_versions = workspace_versions(repository, head)
    comparison, stable_tag = resolve_comparison(repository, base, baseline)
    head_version = head_versions["workspace"]
    if stable_tag is not None and version_tuple(head_version) <= version_tuple(stable_tag.removeprefix("v")):
        raise ReleaseContentError(
            f"release version {head_version} is not newer than stable tag {stable_tag}"
        )
    if stable_tag is None and version_tuple(head_version) != INITIAL_RELEASE_VERSION:
        raise ReleaseContentError(
            f"first product release must be {INITIAL_RELEASE_VERSION[0]}.{INITIAL_RELEASE_VERSION[1]}.{INITIAL_RELEASE_VERSION[2]}"
        )
    if not has_meaningful_content(repository, comparison, head):
        raise ReleaseContentError("release PR contains only generated/version metadata")
    return ReleaseContext(
        repository=repository,
        base=base,
        head=head,
        comparison=comparison,
        version=head_version,
        stable_tag=stable_tag,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", type=Path, default=Path.cwd())
    parser.add_argument("--base", default=os.environ.get("RELEASE_BASE_SHA"))
    parser.add_argument("--head", default=os.environ.get("RELEASE_HEAD_SHA", "HEAD"))
    parser.add_argument("--baseline", default=os.environ.get("RELEASE_BASELINE_SHA"))
    parser.add_argument("--head-branch", default=os.environ.get("RELEASE_HEAD_BRANCH"))
    args = parser.parse_args()
    if not args.base:
        print("release-content: RELEASE_BASE_SHA is required", file=sys.stderr)
        return 1
    try:
        context = validate_context(
            args.repository.resolve(),
            base=args.base,
            head=args.head,
            baseline=args.baseline,
            head_branch=args.head_branch,
        )
    except ReleaseContentError as error:
        print(f"release-content: {error}", file=sys.stderr)
        return 1
    if context is None:
        print("release-content: bypassed for non-release PR")
        return 0
    comparison_label = context.stable_tag or context.comparison
    print(
        f"release-content: valid {context.version} release PR; "
        f"comparison={comparison_label}; head={context.head}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
