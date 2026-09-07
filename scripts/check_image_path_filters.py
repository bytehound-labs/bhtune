#!/usr/bin/env python3
"""Check that image-producing paths stay aligned across GitHub and Woodpecker CI."""

from __future__ import annotations

import re
from pathlib import Path


EXPECTED_PATHS = {
    ".dockerignore",
    ".github/workflows/docker-publish.yml",
    "Cargo.lock",
    "Cargo.toml",
    "Dockerfile",
    "crates/**",
    "frontend/**",
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
}


def _quoted_values(lines: list[str]) -> set[str]:
    values: set[str] = set()
    for line in lines:
        match = re.match(r"^\s*-\s*([\"'])(.+?)\1\s*(?:#.*)?$", line)
        if match:
            values.add(match.group(2))
    return values


def _github_paths(text: str) -> set[str]:
    paths: set[str] = set()
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if re.match(r"^\s{4}paths:\s*$", line):
            block: list[str] = []
            for candidate in lines[index + 1 :]:
                if candidate and len(candidate) - len(candidate.lstrip()) <= 4:
                    break
                block.append(candidate)
            paths.update(_quoted_values(block))
    return paths


def _woodpecker_paths(text: str) -> set[str]:
    match = re.search(
        r"(?ms)^\s*path:\s*\n(?P<block>(?:^[ \t]+.*\n?)*)", text
    )
    if not match:
        raise ValueError("Woodpecker workflow has no path filter")
    return _quoted_values(match.group("block").splitlines())


def main() -> int:
    repository = Path(__file__).resolve().parents[1]
    github_file = repository / ".github" / "workflows" / "docker-publish.yml"
    woodpecker_file = repository / ".woodpecker.yml"

    github_paths = _github_paths(github_file.read_text(encoding="utf-8"))
    woodpecker_paths = _woodpecker_paths(woodpecker_file.read_text(encoding="utf-8"))

    errors: list[str] = []
    if github_paths != EXPECTED_PATHS:
        errors.append(
            "GitHub Actions image paths differ from the expected set: "
            f"{sorted(github_paths ^ EXPECTED_PATHS)}"
        )
    if woodpecker_paths != EXPECTED_PATHS:
        errors.append(
            "Woodpecker image paths differ from the expected set: "
            f"{sorted(woodpecker_paths ^ EXPECTED_PATHS)}"
        )
    if github_paths != woodpecker_paths:
        errors.append(
            "GitHub Actions and Woodpecker image paths differ: "
            f"{sorted(github_paths ^ woodpecker_paths)}"
        )

    if errors:
        for error in errors:
            print(f"error: {error}")
        return 1
    print("Image-producing path filters are aligned.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
