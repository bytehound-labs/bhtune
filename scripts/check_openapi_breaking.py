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
RUNS_PATH = "/api/runs"

# These are deliberate pre-v1 removals: the per-tune quality switch and timing
# settings moved to the TOML-backed global configuration page. Keep this allowlist
# tied to the exact operation and schema so unrelated request-property removals
# remain breaking.
INTENTIONAL_REMOVED_REQUEST_PROPERTIES = frozenset(
    {
        ("POST", RUNS_PATH, "StartRunRequest", "allow_uncertain_quality"),
        ("POST", RUNS_PATH, "StartRunRequest", "mrft_delay"),
        ("POST", RUNS_PATH, "StartRunRequest", "poll_interval_ms"),
        ("POST", RUNS_PATH, "StartRunRequest", "timeout_secs"),
        ("POST", RUNS_PATH, "StartRunRequest", "op_timeout_secs"),
        ("POST", RUNS_PATH, "StartRunRequest", "restore_timeout_secs"),
        ("PUT", "/api/runs/draft", "NewRunDraft", "allow_uncertain_quality"),
    }
)

# This is the deliberate pre-v1 result-validity migration: an invalid calculated
# result has no safe numeric value, so these response fields may become nullable.
# Keep the allowance tied to this one component and these exact properties.
INTENTIONAL_NULLABLE_RESPONSE_PROPERTIES = {
    "ResultResponse": frozenset(
        {
            "kp",
            "ti_minutes",
            "td_minutes",
            "proportional",
            "integral",
            "derivative",
        }
    )
}


def _schema_type(schema: dict[str, Any]) -> str | None:
    return schema.get("type")


def _enum_removed(old: dict[str, Any], new: dict[str, Any]) -> bool:
    old_enum = old.get("enum")
    new_enum = new.get("enum")
    return isinstance(old_enum, list) and isinstance(new_enum, list) and not set(old_enum) <= set(
        new_enum
    )


def _is_nullable_expansion(old: dict[str, Any], new: dict[str, Any]) -> bool:
    old_type = old.get("type")
    new_type = new.get("type")
    return (
        isinstance(old_type, str)
        and isinstance(new_type, list)
        and set(new_type) == {old_type, "null"}
    )


def _schema_header_breaks(old: dict[str, Any], new: dict[str, Any], location: str) -> list[str]:
    errors: list[str] = []
    old_type = _schema_type(old)
    new_type = _schema_type(new)
    if old_type and new_type and old_type != new_type:
        errors.append(f"{location}: schema type changed from {old_type!r} to {new_type!r}")
    if _enum_removed(old, new):
        errors.append(f"{location}: an existing enum value was removed")
    return errors


def _required_breaks(
    old: dict[str, Any],
    new: dict[str, Any],
    direction: str,
    location: str,
    allowed_nullable_properties: frozenset[str],
) -> list[str]:
    old_required = set(old.get("required", []))
    new_required = set(new.get("required", []))
    if direction == "request":
        changed = sorted(new_required - old_required)
        if changed:
            return [f"{location}: request fields became required: {', '.join(changed)}"]
    else:
        changed = sorted(old_required - new_required)
        old_properties = old.get("properties", {})
        new_properties = new.get("properties", {})
        if isinstance(old_properties, dict) and isinstance(new_properties, dict):
            changed = [
                name
                for name in changed
                if not (
                    name in allowed_nullable_properties
                    and isinstance(old_properties.get(name), dict)
                    and isinstance(new_properties.get(name), dict)
                    and _is_nullable_expansion(old_properties[name], new_properties[name])
                )
            ]
        if changed:
            return [f"{location}: response fields stopped being required: {', '.join(changed)}"]
    return []


def _property_break(
    old_property: Any,
    name: str,
    new_properties: dict[str, Any],
    direction: str,
    location: str,
    allowed_removed_properties: frozenset[str],
    allowed_nullable_properties: frozenset[str],
) -> list[str]:
    if name not in new_properties:
        if name in allowed_removed_properties:
            return []
        if direction == "response":
            return [f"{location}: response property {name!r} was removed"]
        return [f"{location}: request property {name!r} was removed"]

    new_property = new_properties[name]
    if not isinstance(old_property, dict) or not isinstance(new_property, dict):
        return []
    if (
        direction == "response"
        and name in allowed_nullable_properties
        and _is_nullable_expansion(old_property, new_property)
    ):
        return []
    return _schema_breaks(
        old_property,
        new_property,
        direction,
        f"{location}.{name}",
        frozenset(),
        frozenset(),
    )


def _property_breaks(
    old_properties: dict[str, Any],
    new_properties: dict[str, Any],
    direction: str,
    location: str,
    allowed_removed_properties: frozenset[str],
    allowed_nullable_properties: frozenset[str],
) -> list[str]:
    errors: list[str] = []
    for name, old_property in old_properties.items():
        errors.extend(
            _property_break(
                old_property,
                name,
                new_properties,
                direction,
                location,
                allowed_removed_properties,
                allowed_nullable_properties,
            )
        )
    return errors


def _schema_breaks(
    old: dict[str, Any],
    new: dict[str, Any],
    direction: str,
    location: str,
    allowed_removed_properties: frozenset[str] = frozenset(),
    allowed_nullable_properties: frozenset[str] = frozenset(),
) -> list[str]:
    errors = _schema_header_breaks(old, new, location)
    errors.extend(
        _required_breaks(
            old,
            new,
            direction,
            location,
            allowed_nullable_properties,
        )
    )

    old_properties = old.get("properties", {})
    new_properties = new.get("properties", {})
    if isinstance(old_properties, dict) and isinstance(new_properties, dict):
        errors.extend(
            _property_breaks(
                old_properties,
                new_properties,
                direction,
                location,
                allowed_removed_properties,
                allowed_nullable_properties,
            )
        )
    return errors


def _content_breaks(
    old_content: dict[str, Any],
    new_content: dict[str, Any],
    direction: str,
    location: str,
    allowed_removed_properties: frozenset[str] = frozenset(),
    allowed_nullable_properties: frozenset[str] = frozenset(),
) -> list[str]:
    errors: list[str] = []
    for content_type, old_media in old_content.items():
        if content_type not in new_content:
            errors.append(f"{location}: content type {content_type!r} was removed")
            continue
        old_schema = old_media.get("schema", {})
        new_schema = new_content[content_type].get("schema", {})
        if isinstance(old_schema, dict) and isinstance(new_schema, dict):
            errors.extend(
                _schema_breaks(
                    old_schema,
                    new_schema,
                    direction,
                    f"{location} {content_type}",
                    allowed_removed_properties,
                    allowed_nullable_properties,
                )
            )
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


def _schema_ref_name(schema: Any) -> str | None:
    if not isinstance(schema, dict):
        return None
    ref = schema.get("$ref")
    if isinstance(ref, str) and ref.startswith("#/components/schemas/"):
        return ref.rsplit("/", 1)[-1]
    return None


def _request_body_allowances(old_body: dict[str, Any], location: str) -> frozenset[str]:
    for old_media in old_body.get("content", {}).values():
        old_schema = old_media.get("schema", {}) if isinstance(old_media, dict) else {}
        schema_name = _schema_ref_name(old_schema)
        if schema_name is not None:
            return frozenset(
                property_name
                for method, path, schema, property_name in INTENTIONAL_REMOVED_REQUEST_PROPERTIES
                if f"{method} {path}" == location and schema == schema_name
            )
    return frozenset()


def _request_body_breaks(old_body: Any, new_body: Any, location: str) -> list[str]:
    if not isinstance(old_body, dict):
        return []
    if not isinstance(new_body, dict):
        return [f"{location}: request body was removed"]

    errors: list[str] = []
    if old_body.get("required") is not True and new_body.get("required") is True:
        errors.append(f"{location}: request body became required")
    errors.extend(
        _content_breaks(
            old_body.get("content", {}),
            new_body.get("content", {}),
            "request",
            f"{location} request body",
            _request_body_allowances(old_body, location),
        )
    )
    return errors


def _response_break(
    status: str,
    old_response: Any,
    new_responses: dict[str, Any],
    location: str,
) -> list[str]:
    if status not in new_responses:
        return [f"{location}: response {status!r} was removed"]
    new_response = new_responses[status]
    if not isinstance(old_response, dict) or not isinstance(new_response, dict):
        return []
    return _content_breaks(
        old_response.get("content", {}),
        new_response.get("content", {}),
        "response",
        f"{location} response {status}",
    )


def _response_breaks(
    old_responses: dict[str, Any], new_responses: dict[str, Any], location: str
) -> list[str]:
    errors: list[str] = []
    for status, old_response in old_responses.items():
        errors.extend(_response_break(status, old_response, new_responses, location))
    return errors


def _security_breaks(old: dict[str, Any], new: dict[str, Any], location: str) -> list[str]:
    if not old.get("security", []) and new.get("security", []):
        return [f"{location}: authentication became mandatory"]
    return []


def _operation_breaks(old: dict[str, Any], new: dict[str, Any], location: str) -> list[str]:
    errors = _parameters_break(old.get("parameters", []), new.get("parameters", []), location)
    errors.extend(_request_body_breaks(old.get("requestBody"), new.get("requestBody"), location))
    errors.extend(_response_breaks(old.get("responses", {}), new.get("responses", {}), location))
    errors.extend(_security_breaks(old, new, location))
    return errors


def _request_schema_names_from_content(content: dict[str, Any]) -> list[str]:
    names: list[str] = []
    for media in content.values():
        schema = media.get("schema", {}) if isinstance(media, dict) else {}
        schema_name = _schema_ref_name(schema)
        if schema_name is not None:
            names.append(schema_name)
    return names


def _request_schema_names(operation: Any) -> list[str]:
    if not isinstance(operation, dict):
        return []
    request_body = operation.get("requestBody", {})
    if not isinstance(request_body, dict):
        return []
    return _request_schema_names_from_content(request_body.get("content", {}))


def _request_schema_refs(paths: dict[str, Any]) -> dict[str, set[tuple[str, str]]]:
    request_refs: dict[str, set[tuple[str, str]]] = {}
    for path, item in paths.items():
        if not isinstance(item, dict):
            continue
        for method in METHODS:
            for schema_name in _request_schema_names(item.get(method)):
                request_refs.setdefault(schema_name, set()).add((method.upper(), path))
    return request_refs


def _component_request_allowances(
    paths: dict[str, Any],
) -> dict[str, frozenset[str]]:
    """Return allowlisted removals only for exact request operations."""
    request_refs = _request_schema_refs(paths)
    allowances: dict[str, frozenset[str]] = {}
    for method, path, schema, property_name in INTENTIONAL_REMOVED_REQUEST_PROPERTIES:
        if (method, path) in request_refs.get(schema, set()):
            allowances[schema] = allowances.get(schema, frozenset()) | frozenset({property_name})
    return allowances


def _method_breaks(
    old_item: dict[str, Any], new_item: dict[str, Any], path: str
) -> list[str]:
    errors: list[str] = []
    for method in METHODS:
        if method not in old_item:
            continue
        location = f"{method.upper()} {path}"
        if method not in new_item:
            errors.append(f"{location}: operation was removed")
            continue
        errors.extend(_operation_breaks(old_item[method], new_item[method], location))
    return errors


def _path_break(
    path: str, old_item: dict[str, Any], new_paths: dict[str, Any]
) -> list[str]:
    if path not in new_paths:
        return [f"path {path!r} was removed"]
    return _method_breaks(old_item, new_paths[path], path)


def _path_breaks(old_paths: dict[str, Any], new_paths: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    for path, old_item in old_paths.items():
        errors.extend(_path_break(path, old_item, new_paths))
    return errors


def _component_break(
    name: str,
    old_schema: Any,
    new_schemas: dict[str, Any],
    request_allowances: dict[str, frozenset[str]],
    response_nullable_allowances: dict[str, frozenset[str]],
) -> list[str]:
    if name not in new_schemas:
        return [f"component schema {name!r} was removed"]
    new_schema = new_schemas[name]
    if not isinstance(old_schema, dict) or not isinstance(new_schema, dict):
        return []
    return _schema_breaks(
        old_schema,
        new_schema,
        "response",
        f"schema {name!r}",
        request_allowances.get(name, frozenset()),
        response_nullable_allowances.get(name, frozenset()),
    )


def _component_breaks(
    old_schemas: dict[str, Any],
    new_schemas: dict[str, Any],
    request_allowances: dict[str, frozenset[str]],
    response_nullable_allowances: dict[str, frozenset[str]],
) -> list[str]:
    errors: list[str] = []
    for name, old_schema in old_schemas.items():
        errors.extend(
            _component_break(
                name,
                old_schema,
                new_schemas,
                request_allowances,
                response_nullable_allowances,
            )
        )
    return errors


def find_breaking_changes(old: dict[str, Any], new: dict[str, Any]) -> list[str]:
    old_paths = old.get("paths", {})
    new_paths = new.get("paths", {})
    errors = _path_breaks(old_paths, new_paths)

    old_schemas = old.get("components", {}).get("schemas", {})
    new_schemas = new.get("components", {}).get("schemas", {})
    response_nullable_allowances = {
        name: properties
        for name, properties in INTENTIONAL_NULLABLE_RESPONSE_PROPERTIES.items()
        if name in old_schemas and name in new_schemas
    }
    errors.extend(
        _component_breaks(
            old_schemas,
            new_schemas,
            _component_request_allowances(old_paths),
            response_nullable_allowances,
        )
    )
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
