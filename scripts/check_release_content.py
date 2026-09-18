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
STABLE_TAG = re.compile(
    r"^v(?P<version>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)$"
)
VERSION_ASSIGNMENT = re.compile(r'^\s*version\s*=\s*(?:"[^"]+"|\{[^}]*\})\s*$')
INLINE_VERSION_ASSIGNMENT = re.compile(
    r'^\s*[A-Za-z0-9_-]+\s*=\s*\{.*\bversion\s*=\s*"[^"]+".*\}\s*$'
)
LOCK_VERSION_LINE = re.compile(r"^\s*version\s*=\s*\"[^\"]+\"\s*$")
CARGO_MANIFEST = "Cargo.toml"
SAFE_GIT_OPTIONS = frozenset(
    {
        "--",
        "--is-ancestor",
        "--list",
        "--merged",
        "--name-only",
        "-e",
        "-n",
    }
)
GIT_REVISION_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,255}$")


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


def _validate_git_revision(value: str, label: str) -> str:
    if (
        not GIT_REVISION_PATTERN.fullmatch(value)
        or ".." in value
        or value.endswith(".")
        or "@{" in value
    ):
        raise ReleaseContentError(f"{label} is not a safe git revision")
    return value


def _validate_repo_path(value: str) -> str:
    path = Path(value)
    if path.is_absolute() or ".." in path.parts or "\x00" in value:
        raise ReleaseContentError(f"repository path is not safe: {value!r}")
    return value


def run_git(repository: Path, *args: str) -> str:
    if not args or args[0] not in {
        "cat-file",
        "diff",
        "merge-base",
        "rev-list",
        "show",
        "tag",
    }:
        raise ReleaseContentError("unsupported git operation")
    for argument in args:
        if "\x00" in argument or "\r" in argument or "\n" in argument:
            raise ReleaseContentError("git argument contains control characters")
        if argument.startswith("-") and argument not in SAFE_GIT_OPTIONS:
            if not argument.startswith("--unified="):
                raise ReleaseContentError(f"unsupported git option: {argument}")
    try:
        result = subprocess.run(
            ["git", *args],
            cwd=repository,
            check=True,
            capture_output=True,
            text=True,
            shell=False,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise ReleaseContentError(f"git {' '.join(args)} failed: {detail.strip()}") from error
    return result.stdout


def parse_version(value: object, label: str) -> tuple[int, int, int]:
    if not isinstance(value, str):
        raise ReleaseContentError(f"{label} is not a version string")
    match = re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", value)
    if not match:
        raise ReleaseContentError(f"{label} must be a stable X.Y.Z version, got {value!r}")
    return tuple(int(part) for part in match.groups())


def read_manifest(repository: Path, revision: str, relative_path: str) -> dict:
    safe_revision = _validate_git_revision(revision, "manifest revision")
    safe_path = _validate_repo_path(relative_path)
    try:
        content = run_git(repository, "show", f"{safe_revision}:{safe_path}")
        return tomllib.loads(content)
    except tomllib.TOMLDecodeError as error:
        raise ReleaseContentError(
            f"{safe_path} at {safe_revision} is malformed TOML: {error}"
        ) from error


def workspace_versions(repository: Path, revision: str) -> dict[str, str]:
    safe_revision = _validate_git_revision(revision, "workspace revision")
    root = read_manifest(repository, safe_revision, CARGO_MANIFEST)
    package = root.get("workspace", {}).get("package", {})
    root_version = package.get("version")
    version_tuple = parse_version(root_version, f"workspace version at {safe_revision}")
    resolved = {"workspace": ".".join(str(part) for part in version_tuple)}
    members = root.get("workspace", {}).get("members")
    if not isinstance(members, list) or set(members) != {
        f"crates/{member}" for member in WORKSPACE_MEMBERS
    }:
        raise ReleaseContentError(
            f"workspace members at {safe_revision} do not match the five BHTune crates"
        )

    for member in WORKSPACE_MEMBERS:
        path = f"crates/{member}/{CARGO_MANIFEST}"
        manifest = read_manifest(repository, safe_revision, path)
        package_table = manifest.get("package")
        if not isinstance(package_table, dict):
            raise ReleaseContentError(f"{path} at {safe_revision} has no [package] table")
        member_version = package_table.get("version")
        if isinstance(member_version, dict) and member_version.get("workspace") is True:
            resolved[member] = resolved["workspace"]
        elif isinstance(member_version, str):
            parse_version(member_version, f"{path} version at {safe_revision}")
            resolved[member] = member_version
        else:
            raise ReleaseContentError(
                f"{path} at {safe_revision} has no resolvable package version"
            )
        if resolved[member] != resolved["workspace"]:
            raise ReleaseContentError(
                f"{path} resolves to {resolved[member]}, not workspace version {resolved['workspace']}"
            )
    return resolved


def version_tuple(value: str) -> tuple[int, int, int]:
    return parse_version(value, "version")


def stable_tags(repository: Path, base: str) -> list[tuple[tuple[int, int, int], str]]:
    safe_base = _validate_git_revision(base, "comparison base")
    tags = run_git(repository, "tag", "--merged", safe_base, "--list", "v*").splitlines()
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
    safe_baseline = _validate_git_revision(baseline, "release baseline")
    safe_base = _validate_git_revision(base, "comparison base")
    run_git(repository, "cat-file", "-e", f"{safe_baseline}^{{commit}}")
    try:
        run_git(repository, "merge-base", "--is-ancestor", safe_baseline, safe_base)
    except ReleaseContentError as error:
        raise ReleaseContentError("RELEASE_BASELINE_SHA is not an ancestor of the PR base") from error
    return safe_baseline, None


def diff_files(repository: Path, comparison: str, head: str) -> list[str]:
    safe_comparison = _validate_git_revision(comparison, "comparison revision")
    safe_head = _validate_git_revision(head, "head revision")
    output = run_git(repository, "diff", "--name-only", f"{safe_comparison}..{safe_head}")
    return [line for line in output.splitlines() if line]


def diff_lines(repository: Path, comparison: str, head: str, path: str) -> list[str]:
    safe_comparison = _validate_git_revision(comparison, "comparison revision")
    safe_head = _validate_git_revision(head, "head revision")
    safe_path = _validate_repo_path(path)
    output = run_git(
        repository,
        "diff",
        "--unified=0",
        f"{safe_comparison}..{safe_head}",
        "--",
        safe_path,
    )
    return [
        line[1:]
        for line in output.splitlines()
        if line.startswith(("+", "-")) and not line.startswith(("+++", "---"))
    ]


def version_metadata_only(path: str, lines: Iterable[str]) -> bool:
    if path.endswith("Cargo.lock"):
        return all(LOCK_VERSION_LINE.fullmatch(line) for line in lines)
    if path == CARGO_MANIFEST or path.startswith("crates/") and path.endswith(CARGO_MANIFEST):
        return all(
            VERSION_ASSIGNMENT.fullmatch(line) or INLINE_VERSION_ASSIGNMENT.fullmatch(line)
            for line in lines
        )
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
