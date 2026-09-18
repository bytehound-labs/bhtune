"""Create and validate exact-version Docusaurus documentation snapshots."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import Any, Iterable

from check_release_content import ReleaseContentError, parse_version

DOCS_DIR = Path("docs")
WEBSITE_DIR = Path("website")
VERSIONED_DOCS_DIR = WEBSITE_DIR / "versioned_docs"
VERSIONED_SIDEBARS_DIR = WEBSITE_DIR / "versioned_sidebars"
VERSIONS_FILE = WEBSITE_DIR / "versions.json"
DIGEST_FILE = ".release-docs-digest"


class DocsVersionError(RuntimeError):
    """Raised when a documentation snapshot is invalid or cannot be written."""


def workspace_version(repository: Path) -> str:
    try:
        data = tomllib.loads((repository / "Cargo.toml").read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise DocsVersionError(f"could not read workspace version: {error}") from error
    try:
        return data["workspace"]["package"]["version"]
    except (KeyError, TypeError) as error:
        raise DocsVersionError("Cargo.toml has no [workspace.package].version") from error


def stable_version(value: str) -> tuple[int, int, int] | None:
    try:
        return parse_version(value, "documentation version")
    except ReleaseContentError:
        return None


def version_dir(version: str) -> Path:
    return VERSIONED_DOCS_DIR / f"version-{version}"


def sidebar_path(version: str) -> Path:
    return VERSIONED_SIDEBARS_DIR / f"version-{version}-sidebars.json"


def iter_source_files(repository: Path) -> Iterable[Path]:
    docs_root = repository / DOCS_DIR
    if not docs_root.is_dir():
        raise DocsVersionError(f"documentation source directory is missing: {docs_root}")
    for path in sorted(docs_root.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(docs_root)
        if relative.parts and relative.parts[0] == "internal":
            continue
        yield path


def digest_inputs(repository: Path) -> str:
    digest = hashlib.sha256()
    for path in iter_source_files(repository):
        relative = path.relative_to(repository).as_posix()
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    sidebars = repository / WEBSITE_DIR / "sidebars.ts"
    if not sidebars.is_file():
        raise DocsVersionError(f"sidebar source is missing: {sidebars}")
    digest.update(b"website/sidebars.ts\0")
    digest.update(sidebars.read_bytes())
    return digest.hexdigest()


def copy_sources(repository: Path, destination: Path) -> None:
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True, exist_ok=True)
    for source in iter_source_files(repository):
        relative = source.relative_to(repository / DOCS_DIR)
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)


def source_doc_paths(repository: Path) -> list[Path]:
    return [
        path.relative_to(repository / DOCS_DIR)
        for path in iter_source_files(repository)
        if path.suffix.lower() in {".md", ".mdx"}
    ]


def category_label(path: Path) -> str:
    category_file = path / "_category_.json"
    if category_file.is_file():
        try:
            data = json.loads(category_file.read_text(encoding="utf-8"))
            if isinstance(data, dict) and isinstance(data.get("label"), str):
                return data["label"]
        except json.JSONDecodeError as error:
            raise DocsVersionError(f"malformed category metadata: {category_file}: {error}") from error
    return path.name.replace("-", " ").replace("_", " ").title()


def frontmatter_position(path: Path) -> tuple[int, str]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise DocsVersionError(f"could not read documentation file {path}: {error}") from error
    if not text.startswith("---\n"):
        return (10_000, path.name)
    end = text.find("\n---", 4)
    if end < 0:
        return (10_000, path.name)
    for line in text[4:end].splitlines():
        if line.startswith("sidebar_position:"):
            try:
                return (int(line.split(":", 1)[1].strip()), path.name)
            except ValueError as error:
                raise DocsVersionError(f"invalid sidebar_position in {path}") from error
    return (10_000, path.name)


def sidebar_items(repository: Path, directory: Path) -> list[Any]:
    entries: list[tuple[tuple[int, str], Any]] = []
    children = sorted(path for path in directory.iterdir() if path.name != "internal")
    files = [path for path in children if path.is_file() and path.suffix.lower() in {".md", ".mdx"}]
    for path in files:
        relative = path.relative_to(repository / DOCS_DIR).with_suffix("")
        item = relative.as_posix()
        entries.append((frontmatter_position(path), item))
    for path in children:
        if not path.is_dir() or path.name.startswith("."):
            continue
        if not any(path.rglob("*.md")) and not any(path.rglob("*.mdx")):
            continue
        nested = sidebar_items(repository, path)
        entries.append(
            (
                (10_000, path.name),
                {"type": "category", "label": category_label(path), "items": nested},
            )
        )
    entries.sort(key=lambda entry: entry[0])
    return [item for _, item in entries]


def render_sidebar(repository: Path) -> dict[str, list[Any]]:
    return {"docsSidebar": sidebar_items(repository, repository / DOCS_DIR)}


def read_versions(repository: Path) -> list[str]:
    path = repository / VERSIONS_FILE
    if not path.exists():
        return []
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise DocsVersionError(f"malformed {VERSIONS_FILE}: {error}") from error
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise DocsVersionError(f"{VERSIONS_FILE} must contain an array of versions")
    versions = []
    for item in value:
        if stable_version(item) is None:
            raise DocsVersionError(f"{VERSIONS_FILE} contains a non-stable version: {item}")
        if item not in versions:
            versions.append(item)
    return versions


def validate_snapshot(repository: Path, version: str, versions: list[str]) -> None:
    root = repository / version_dir(version)
    if not root.is_dir():
        raise DocsVersionError(f"missing versioned documentation directory: {root}")
    digest_path = root / DIGEST_FILE
    if not digest_path.is_file():
        raise DocsVersionError(f"missing snapshot digest: {digest_path}")
    expected_digest = digest_inputs(repository)
    if digest_path.read_text(encoding="utf-8").strip() != expected_digest:
        raise DocsVersionError(f"documentation snapshot is stale for version {version}")
    expected_sidebar = render_sidebar(repository)
    sidebar = repository / sidebar_path(version)
    if not sidebar.is_file():
        raise DocsVersionError(f"missing versioned sidebar: {sidebar}")
    try:
        actual_sidebar = json.loads(sidebar.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise DocsVersionError(f"malformed versioned sidebar: {sidebar}: {error}") from error
    if actual_sidebar != expected_sidebar:
        raise DocsVersionError(f"versioned sidebar is stale for version {version}")
    expected_files = {
        path.relative_to(repository / DOCS_DIR).as_posix()
        for path in iter_source_files(repository)
    }
    actual_files = {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file() and path.name != DIGEST_FILE
    }
    if actual_files != expected_files:
        raise DocsVersionError(f"versioned documentation tree is stale for version {version}")
    for relative in expected_files:
        source = repository / DOCS_DIR / relative
        snapshot = root / relative
        if snapshot.read_bytes() != source.read_bytes():
            raise DocsVersionError(f"versioned documentation content is stale for version {version}")
    if len(versions) > 3 or versions != sorted(versions, key=lambda item: stable_version(item), reverse=True):
        raise DocsVersionError(f"{VERSIONS_FILE} is not sorted newest-first")
    if version not in versions[:3]:
        raise DocsVersionError(f"version {version} is not retained in {VERSIONS_FILE}")


def _is_stale_version(name: str, retained: set[str], prefix: str, suffix: str = "") -> bool:
    if not name.startswith(prefix) or suffix and not name.endswith(suffix):
        return False
    version = name.removeprefix(prefix)
    if suffix:
        version = version.removesuffix(suffix)
    return stable_version(version) is not None and version not in retained


def _remove_stale_doc_versions(root: Path, retained: set[str]) -> None:
    if not root.is_dir():
        return
    for path in root.iterdir():
        if path.is_dir() and _is_stale_version(path.name, retained, "version-"):
            shutil.rmtree(path)


def _remove_stale_sidebars(root: Path, retained: set[str]) -> None:
    if not root.is_dir():
        return
    for path in root.iterdir():
        if path.is_file() and _is_stale_version(
            path.name,
            retained,
            "version-",
            "-sidebars.json",
        ):
            path.unlink()


def remove_exact_stale_versions(repository: Path, retained: list[str]) -> None:
    retained_set = set(retained)
    _remove_stale_doc_versions(repository / VERSIONED_DOCS_DIR, retained_set)
    _remove_stale_sidebars(repository / VERSIONED_SIDEBARS_DIR, retained_set)


def _snapshot_matches(repository: Path, version: str, digest: str, sidebar_value: dict) -> bool:
    destination = repository / version_dir(version)
    current_digest = (
        (destination / DIGEST_FILE).read_text(encoding="utf-8").strip()
        if (destination / DIGEST_FILE).is_file()
        else None
    )
    sidebar = repository / sidebar_path(version)
    current_sidebar = None
    if sidebar.is_file():
        try:
            current_sidebar = json.loads(sidebar.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            current_sidebar = None
    expected_files = {
        path.relative_to(repository / DOCS_DIR).as_posix()
        for path in iter_source_files(repository)
    }
    actual_files = (
        {
            path.relative_to(destination).as_posix()
            for path in destination.rglob("*")
            if path.is_file() and path.name != DIGEST_FILE
        }
        if destination.is_dir()
        else set()
    )
    files_match = actual_files == expected_files and all(
        (destination / relative).read_bytes() == (repository / DOCS_DIR / relative).read_bytes()
        for relative in expected_files
    )
    return current_digest == digest and current_sidebar == sidebar_value and files_match


def _synchronize_snapshot(
    repository: Path,
    version: str,
    digest: str,
    sidebar_value: dict,
) -> bool:
    if _snapshot_matches(repository, version, digest, sidebar_value):
        return False
    destination = repository / version_dir(version)
    copy_sources(repository, destination)
    (destination / DIGEST_FILE).write_text(digest + "\n", encoding="utf-8")
    sidebar = repository / sidebar_path(version)
    sidebar.parent.mkdir(parents=True, exist_ok=True)
    sidebar.write_text(json.dumps(sidebar_value, indent=2) + "\n", encoding="utf-8")
    return True


def _retained_versions(repository: Path, version: str) -> tuple[list[str], list[str]]:
    existing = read_versions(repository)
    versions = [item for item in existing if item != version]
    versions.append(version)
    versions.sort(key=lambda item: stable_version(item), reverse=True)
    return existing, versions[:3]


def synchronize(repository: Path, version: str) -> bool:
    repository = repository.resolve()
    if stable_version(version) is None:
        return False
    existing_versions, retained = _retained_versions(repository, version)
    changed = _synchronize_snapshot(
        repository,
        version,
        digest_inputs(repository),
        render_sidebar(repository),
    )
    if existing_versions != retained:
        versions_path = (repository / VERSIONS_FILE).resolve()
        try:
            versions_path.relative_to(repository)
        except ValueError as error:
            raise DocsVersionError(
                f"version metadata path escapes repository: {versions_path}"
            ) from error
        versions_path.parent.mkdir(parents=True, exist_ok=True)
        versions_path.write_text(json.dumps(retained, indent=2) + "\n", encoding="utf-8")
        changed = True
    remove_exact_stale_versions(repository, retained)
    validate_snapshot(repository, version, retained)
    return changed


def check(repository: Path, version: str) -> bool:
    if stable_version(version) is None:
        return False
    versions = read_versions(repository)
    validate_snapshot(repository, version, versions)
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", type=Path, default=Path.cwd())
    parser.add_argument("--version", default=os.environ.get("RELEASE_VERSION"))
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    repository = args.repository.resolve()
    try:
        version = args.version or workspace_version(repository)
        result = check(repository, version) if args.check else synchronize(repository, version)
    except DocsVersionError as error:
        print(f"docs-version: {error}", file=sys.stderr)
        return 1
    if result:
        print(f"docs-version: {'validated' if args.check else 'synchronized'} {version}")
    else:
        print(f"docs-version: skipped prerelease {version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
