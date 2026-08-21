#!/usr/bin/env python3

import unittest

from check_openapi_breaking import find_breaking_changes


def spec(*, required=False, include_path=True):
    paths = {}
    if include_path:
        paths["/api/example"] = {
            "post": {
                "requestBody": {
                    "required": False,
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {"name": {"type": "string"}},
                                "required": ["name"] if required else [],
                            }
                        }
                    },
                },
                "responses": {
                    "200": {
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": {"id": {"type": "integer"}},
                                    "required": ["id"],
                                }
                            }
                        }
                    }
                },
            }
        }
    return {
        "openapi": "3.1.0",
        "paths": paths,
        "components": {"schemas": {"Example": {"type": "object", "properties": {}}}},
    }


class OpenApiBreakingTests(unittest.TestCase):
    def test_identical_specs_are_compatible(self):
        self.assertEqual(find_breaking_changes(spec(), spec()), [])

    def test_removed_path_is_breaking(self):
        errors = find_breaking_changes(spec(), spec(include_path=False))
        self.assertTrue(any("path" in error for error in errors))

    def test_new_required_request_field_is_breaking(self):
        errors = find_breaking_changes(spec(), spec(required=True))
        self.assertTrue(any("became required" in error for error in errors))

    def test_removed_response_property_is_breaking(self):
        old = spec()
        new = spec()
        del new["paths"]["/api/example"]["post"]["responses"]["200"]["content"][
            "application/json"
        ]["schema"]["properties"]["id"]
        self.assertTrue(any("response property" in error for error in find_breaking_changes(old, new)))

    def test_removed_component_schema_is_breaking(self):
        old = spec()
        new = spec()
        del new["components"]["schemas"]["Example"]
        self.assertTrue(any("component schema" in error for error in find_breaking_changes(old, new)))


if __name__ == "__main__":
    unittest.main()
