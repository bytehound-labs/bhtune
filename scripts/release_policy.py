"""Release-plz commit policy helpers.

The release workflow deliberately keeps this policy outside YAML so it can be
tested without a GitHub runner and shared by release-plz configuration checks.
"""

from __future__ import annotations

import re

RELEASE_WORTHY_TYPES = frozenset(
    {"feat", "fix", "perf", "refactor", "docs", "test", "build", "ci", "revert"}
)

_VERSION = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?"
_RELEASE_SUBJECTS = (
    re.compile(rf"^chore\(release\): prepare v{_VERSION}$"),
    re.compile(rf"^chore: release v{_VERSION}$"),
)
_CONVENTIONAL_SUBJECT = re.compile(
    r"^(?P<type>[a-z]+)(?:\([^()\r\n]+\))?!?:\s+(?P<description>\S.*)$"
)


def is_release_commit(subject: str) -> bool:
    """Return whether *subject* is one of the release-plz commit shapes."""

    return any(pattern.fullmatch(subject.strip()) for pattern in _RELEASE_SUBJECTS)


def conventional_type(subject: str) -> str | None:
    """Return a conventional-commit type, or ``None`` for malformed input."""

    match = _CONVENTIONAL_SUBJECT.fullmatch(subject.strip())
    return match.group("type") if match else None


def is_release_worthy(subject: str) -> bool:
    """Return whether a commit subject should trigger a product release."""

    subject = subject.strip()
    if not subject or is_release_commit(subject):
        return False
    commit_type = conventional_type(subject)
    return commit_type in RELEASE_WORTHY_TYPES


def release_commits_regex() -> str:
    """Return the regex used by release-plz to gate release proposals."""

    return r"^(feat|fix|perf|refactor|docs|test|build|ci|revert)(\([^)\r\n]+\))?!?:\s+\S"


def main() -> int:
    """Run the small command-line predicate used by shell workflows."""

    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release-commit-subject")
    parser.add_argument("--release-worthy-subject")
    args = parser.parse_args()

    if args.release_commit_subject is not None:
        return 0 if is_release_commit(args.release_commit_subject) else 1
    if args.release_worthy_subject is not None:
        return 0 if is_release_worthy(args.release_worthy_subject) else 1
    parser.error("one predicate argument is required")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
