#!/usr/bin/env python3

"""Validate BHTune's single-product release-plz configuration."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path


class ReleasePlzConfigError(ValueError):
    """Raised when release-plz configuration violates the BHTune release policy."""


WORKSPACE_PACKAGES = {
    "bhtune-core",
    "bhtune-driver",
    "bhtune-db",
    "bhtune-cli",
    "bhtune-server",
}
ANCHOR_PACKAGE = "bhtune-cli"
ANCHOR_CONTEXT = f"package.{ANCHOR_PACKAGE}"
RELEASE_COMMIT_PATTERN = (
    r"^(feat|fix|perf|refactor|docs|test|build|ci|revert)"
    r"(\([^)]*\))?!?:\s+\S"
)


def _require(mapping: dict, key: str, expected, context: str) -> None:
    actual = mapping.get(key)
    if actual != expected:
        raise ReleasePlzConfigError(
            f"{context}.{key} must be {expected!r}, got {actual!r}"
        )


def _effective(mapping: dict, workspace: dict, key: str):
    return mapping[key] if key in mapping else workspace.get(key)


def _safe_config_path(path: Path) -> Path:
    if ".." in path.parts or "\x00" in str(path):
        raise ReleasePlzConfigError(f"configuration path is not safe: {path}")
    return path.resolve()


def _load_config(path: Path) -> tuple[dict, str]:
    try:
        safe_path = _safe_config_path(path)
        raw = safe_path.read_bytes()
        source = raw.decode("utf-8")
        config = tomllib.loads(source)
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise ReleasePlzConfigError(f"cannot parse {path}: {exc}") from exc
    return config, source


def _validate_workspace(workspace: object) -> dict:
    if not isinstance(workspace, dict):
        raise ReleasePlzConfigError("[workspace] is required")

    _require(workspace, "publish", False, "workspace")
    _require(workspace, "git_only", True, "workspace")
    _require(workspace, "release", False, "workspace")
    _require(workspace, "git_tag_enable", False, "workspace")
    _require(workspace, "git_release_enable", False, "workspace")
    _require(workspace, "changelog_update", False, "workspace")
    _require(workspace, "pr_name", "chore(release): prepare v{{ version }}", "workspace")
    _require(workspace, "pr_branch_prefix", "release-plz-", "workspace")

    release_commits = workspace.get("release_commits")
    if release_commits != RELEASE_COMMIT_PATTERN:
        raise ReleasePlzConfigError(
            "workspace.release_commits does not match the shared release commit policy"
        )
    return workspace


def _package_map(packages: object) -> dict[str, dict]:
    if not isinstance(packages, list):
        raise ReleasePlzConfigError("at least one [[package]] entry is required")

    package_map: dict[str, dict] = {}
    for package in packages:
        if not isinstance(package, dict) or not isinstance(package.get("name"), str):
            raise ReleasePlzConfigError("every [[package]] entry needs a string name")
        name = package["name"]
        if name in package_map:
            raise ReleasePlzConfigError(f"duplicate [[package]] entry for {name!r}")
        package_map[name] = package
    return package_map


def _validate_anchor(package_map: dict[str, dict], workspace: dict) -> None:
    if set(package_map) != {ANCHOR_PACKAGE}:
        raise ReleasePlzConfigError(
            "only bhtune-cli may override the workspace release configuration"
        )

    anchor = package_map[ANCHOR_PACKAGE]
    _require(anchor, "release", True, ANCHOR_CONTEXT)
    for key, expected in (
        ("publish", False),
        ("git_only", True),
        ("git_release_enable", False),
    ):
        actual = _effective(anchor, workspace, key)
        if actual != expected:
            raise ReleasePlzConfigError(
                f"{ANCHOR_CONTEXT} effective {key} must be {expected!r}, got {actual!r}"
            )
    _require(anchor, "git_tag_enable", True, ANCHOR_CONTEXT)
    _require(anchor, "git_tag_name", "v{{ version }}", ANCHOR_CONTEXT)
    _require(anchor, "changelog_update", True, ANCHOR_CONTEXT)
    _require(anchor, "changelog_path", "CHANGELOG.md", ANCHOR_CONTEXT)

    included = anchor.get("changelog_include")
    if set(included or ()) != WORKSPACE_PACKAGES - {"bhtune-cli"}:
        raise ReleasePlzConfigError(
            f"{ANCHOR_CONTEXT}.changelog_include must contain the other four workspace crates"
        )


def _reject_registry_tokens(source: str) -> None:
    if re.search(r"(?im)^\s*(?:token|registry_token|cargo_registry_token)\s*=", source):
        raise ReleasePlzConfigError("release-plz configuration must not contain registry tokens")
    if "CARGO_REGISTRY_TOKEN" in source or "CRATES_IO_TOKEN" in source:
        raise ReleasePlzConfigError("release-plz configuration must not reference registry tokens")


def validate_config(path: Path) -> dict:
    """Load and validate a release-plz TOML configuration."""
    config, source = _load_config(path)
    workspace = _validate_workspace(config.get("workspace"))
    package_map = _package_map(config.get("package"))
    _validate_anchor(package_map, workspace)
    _reject_registry_tokens(source)
    return config


def main() -> int:
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("release-plz.toml")
    try:
        validate_config(path)
    except ReleasePlzConfigError as exc:
        print(f"release-plz configuration invalid: {exc}", file=sys.stderr)
        return 1
    print(f"release-plz configuration valid: {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
