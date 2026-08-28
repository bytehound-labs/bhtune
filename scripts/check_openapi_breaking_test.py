#!/usr/bin/env python3

import unittest

from check_openapi_breaking import find_breaking_changes

JSON_CONTENT_TYPE = "application/json"
RUNS_PATH = "/api/runs"


def spec(*, required=False, include_path=True):
    paths = {}
    if include_path:
        paths["/api/example"] = {
            "post": {
                "requestBody": {
                    "required": False,
                    "content": {
                        JSON_CONTENT_TYPE: {
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
                            JSON_CONTENT_TYPE: {
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


def spec_with_quality_field(*, remove_quality=False, remove_unrelated=False):
    request_properties = {
        "allow_uncertain_quality": {"type": "boolean"},
        "tagname": {"type": "string"},
    }
    if remove_quality:
        del request_properties["allow_uncertain_quality"]
    if remove_unrelated:
        del request_properties["tagname"]
    schema = {
        "type": "object",
        "properties": request_properties,
        "required": ["tagname"],
    }
    return {
        "openapi": "3.1.0",
        "paths": {
            RUNS_PATH: {
                "post": {
                    "requestBody": {
                        "required": True,
                        "content": {
                            JSON_CONTENT_TYPE: {
                                "schema": {"$ref": "#/components/schemas/StartRunRequest"}
                            }
                        },
                    },
                    "responses": {"200": {"description": "ok"}},
                }
            }
        },
        "components": {"schemas": {"StartRunRequest": schema}},
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
            JSON_CONTENT_TYPE
        ]["schema"]["properties"]["id"]
        self.assertTrue(any("response property" in error for error in find_breaking_changes(old, new)))

    def test_removed_component_schema_is_breaking(self):
        old = spec()
        new = spec()
        del new["components"]["schemas"]["Example"]
        self.assertTrue(any("component schema" in error for error in find_breaking_changes(old, new)))

    def test_intentional_quality_request_removal_is_allowed(self):
        self.assertEqual(
            find_breaking_changes(spec_with_quality_field(), spec_with_quality_field(remove_quality=True)),
            [],
        )

    def test_unrelated_request_property_removal_remains_breaking(self):
        errors = find_breaking_changes(
            spec_with_quality_field(), spec_with_quality_field(remove_unrelated=True)
        )
        self.assertTrue(any("'tagname' was removed" in error for error in errors))

    def test_quality_removal_on_another_operation_remains_breaking(self):
        old = spec_with_quality_field()
        new = spec_with_quality_field(remove_quality=True)
        old["paths"]["/api/other"] = old["paths"].pop(RUNS_PATH)
        new["paths"]["/api/other"] = new["paths"].pop(RUNS_PATH)
        errors = find_breaking_changes(old, new)
        self.assertTrue(any("'allow_uncertain_quality' was removed" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
