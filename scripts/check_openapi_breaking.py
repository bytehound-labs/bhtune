#!/usr/bin/env python3
"""Detect source-compatible OpenAPI changes that require a client or server update.

This is intentionally a small, dependency-free check for the compatibility rules that matter
to BHTune's generated clients. It is not a replacement for full OpenAPI validation or a
general-purpose diff viewer: generated-spec drift remains a separate check.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Iterable
from pathlib import Path
from typing import Any

METHODS = ("get", "put", "post", "delete", "options", "head", "patch", "trace")


def _schema_type(schema: dict[str, Any]) -> str | None:
    return schema.get("type")


def _enum_removed(old: dict[str, Any], new: dict[str, Any]) -> bool:
    old_enum = old.get("enum")
    new_enum = new.get("enum")
    return isinstance(old_enum, list) and isinstance(new_enum, list) and not set(old_enum) <= set(
        new_enum
    )


def _schema_breaks(
    old: dict[str, Any], new: dict[str, Any], direction: str, location: str
) -> list[str]:
    errors: list[str] = []
    if _schema_type(old) != _schema_type(new) and _schema_type(old) and _schema_type(new):
        errors.append(f"{location}: schema type changed from {_schema_type(old)!r} to {_schema_type(new)!r}")
    if _enum_removed(old, new):
        errors.append(f"{location}: an existing enum value was removed")

    old_required = set(old.get("required", []))
    new_required = set(new.get("required", []))
    if direction == "request":
        added_required = sorted(new_required - old_required)
        if added_required:
            errors.append(f"{location}: request fields became required: {', '.join(added_required)}")
    else:
        removed_required = sorted(old_required - new_required)
        if removed_required:
            errors.append(f"{location}: response fields stopped being required: {', '.join(removed_required)}")

    old_properties = old.get("properties", {})
    new_properties = new.get("properties", {})
    if isinstance(old_properties, dict) and isinstance(new_properties, dict):
        for name, old_property in old_properties.items():
            if name not in new_properties and direction == "response":
                errors.append(f"{location}: response property {name!r} was removed")
            elif isinstance(old_property, dict) and isinstance(new_properties.get(name), dict):
                errors.extend(
                    _schema_breaks(
                        old_property,
                        new_properties[name],
                        direction,
                        f"{location}.{name}",
                    )
                )
    return errors


def _content_breaks(
    old_content: dict[str, Any], new_content: dict[str, Any], direction: str, location: str
) -> list[str]:
    errors: list[str] = []
    for content_type, old_media in old_content.items():
        if content_type not in new_content:
            errors.append(f"{location}: content type {content_type!r} was removed")
            continue
        old_schema = old_media.get("schema", {})
        new_schema = new_content[content_type].get("schema", {})
        if isinstance(old_schema, dict) and isinstance(new_schema, dict):
            errors.extend(_schema_breaks(old_schema, new_schema, direction, f"{location} {content_type}"))
    return errors


def _parameters_break(
    old_parameters: Iterable[dict[str, Any]],
    new_parameters: Iterable[dict[str, Any]],
    location: str,
) -> list[str]:
    old_by_key = {(item.get("in"), item.get("name")): item for item in old_parameters}
    new_by_key = {(item.get("in"), item.get("name")): item for item in new_parameters}
    errors: list[str] = []
    for key, old_parameter in old_by_key.items():
        if key not in new_by_key:
            errors.append(f"{location}: parameter {key!r} was removed")
            continue
        new_parameter = new_by_key[key]
        if old_parameter.get("required") is not True and new_parameter.get("required") is True:
            errors.append(f"{location}: parameter {key!r} became required")
        old_schema = old_parameter.get("schema", {})
        new_schema = new_parameter.get("schema", {})
        if isinstance(old_schema, dict) and isinstance(new_schema, dict):
            errors.extend(_schema_breaks(old_schema, new_schema, "request", f"{location} {key!r}"))
    return errors


def _operation_breaks(old: dict[str, Any], new: dict[str, Any], location: str) -> list[str]:
    errors = _parameters_break(
        old.get("parameters", []),
        new.get("parameters", []),
        location,
    )

    old_body = old.get("requestBody")
    new_body = new.get("requestBody")
    if isinstance(old_body, dict):
        if not isinstance(new_body, dict):
            errors.append(f"{location}: request body was removed")
        else:
            if old_body.get("required") is not True and new_body.get("required") is True:
                errors.append(f"{location}: request body became required")
            errors.extend(
                _content_breaks(
                    old_body.get("content", {}),
                    new_body.get("content", {}),
                    "request",
                    f"{location} request body",
                )
            )

    old_responses = old.get("responses", {})
    new_responses = new.get("responses", {})
    for status, old_response in old_responses.items():
        if status not in new_responses:
            errors.append(f"{location}: response {status!r} was removed")
            continue
        new_response = new_responses[status]
        if isinstance(old_response, dict) and isinstance(new_response, dict):
            errors.extend(
                _content_breaks(
                    old_response.get("content", {}),
                    new_response.get("content", {}),
                    "response",
                    f"{location} response {status}",
                )
            )

    old_security = old.get("security", [])
    new_security = new.get("security", [])
    if not old_security and new_security:
        errors.append(f"{location}: authentication became mandatory")
    return errors


def find_breaking_changes(old: dict[str, Any], new: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    old_paths = old.get("paths", {})
    new_paths = new.get("paths", {})
    for path, old_item in old_paths.items():
        if path not in new_paths:
            errors.append(f"path {path!r} was removed")
            continue
        new_item = new_paths[path]
        for method in METHODS:
            if method not in old_item:
                continue
            if method not in new_item:
                errors.append(f"{method.upper()} {path}: operation was removed")
                continue
            errors.extend(_operation_breaks(old_item[method], new_item[method], f"{method.upper()} {path}"))

    old_schemas = old.get("components", {}).get("schemas", {})
    new_schemas = new.get("components", {}).get("schemas", {})
    for name, old_schema in old_schemas.items():
        if name not in new_schemas:
            errors.append(f"component schema {name!r} was removed")
            continue
        if isinstance(old_schema, dict) and isinstance(new_schemas[name], dict):
            errors.extend(_schema_breaks(old_schema, new_schemas[name], "response", f"schema {name!r}"))
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", type=Path, help="baseline OpenAPI JSON")
    parser.add_argument("revision", type=Path, help="revision OpenAPI JSON")
    args = parser.parse_args(argv)

    with args.base.open(encoding="utf-8") as handle:
        old = json.load(handle)
    with args.revision.open(encoding="utf-8") as handle:
        new = json.load(handle)

    errors = find_breaking_changes(old, new)
    if errors:
        print("Breaking OpenAPI changes detected:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("No breaking OpenAPI changes detected.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
