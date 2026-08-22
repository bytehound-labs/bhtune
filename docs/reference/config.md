# Configuration reference

Generated from the real `BhtuneConfig`/`DcsTemplate` Rust types (`schemars`) -- never hand-edit this file, run `cargo run -p bhtune-cli --example gen_docs --features schemars` instead. See `docs/dcs-templates.md` for a worked, prose explanation of the template fields; this page is the exhaustive machine-checked contract both formats must satisfy.

## `bhtune.toml`

JSON Schema for bhtune's TOML config file (`crate::config::BhtuneConfig` in `bhtune-cli`). Every field is optional -- see `AGENTS.md`'s `cli-config` notes for the full `CLI flag > env var > TOML config file > built-in default` precedence each one resolves through.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "BhtuneConfig",
  "description": "bhtune's configuration, loaded from an optional TOML file. Every field is optional; a\nvalue missing from the file (or the file itself missing) falls back to the env var / CLI\nflag / built-in default resolution in the `resolve_*` functions below.",
  "type": "object",
  "properties": {
    "bind": {
      "description": "Overrides [`DEFAULT_BIND_ADDR`] -- the `host:port` `bhtune-server` listens on. Only\nmeaningful to the server binary; see [`resolve_bind_addr`].",
      "type": [
        "string",
        "null"
      ]
    },
    "bridge_host": {
      "description": "Overrides [`DEFAULT_BRIDGE_HOST`] for every `tune --driver opcda` and `opc`\nsubcommand invocation that doesn't pass `--bridge-host` explicitly.",
      "type": [
        "string",
        "null"
      ]
    },
    "db": {
      "description": "Overrides the default SQLite database path (see [`default_db_path_from`]).",
      "type": [
        "string",
        "null"
      ]
    },
    "log": {
      "description": "`[log]` sub-table: level/directory/format/rotation for `crate::logging`'s tracing\nsetup, mirroring `opcda-bridge-gateway`'s own `log.*` config conventions.",
      "$ref": "#/$defs/LogConfig",
      "default": {
        "dir": null,
        "format": null,
        "level": null,
        "rotation": null
      }
    },
    "retention_days": {
      "description": "Age-based history retention (`history-retention`): tune runs with `started_at` older\nthan this many days are deleted automatically on every startup (both binaries, via\n`crate::db::open`) and, for `bhtune-server`, again on a periodic timer while it keeps\nrunning -- see `crate::retention`. `None` (the default) means retain forever: there is\nno built-in number of days, since at this project's data volumes (see AGENTS.md's\nHistory explorer notes) an unexpected auto-delete of someone's baseline tune is a\nworse failure mode than an ever-growing database file. See [`resolve_retention_days`].",
      "type": [
        "integer",
        "null"
      ],
      "format": "uint32",
      "minimum": 0
    },
    "server": {
      "description": "Default OPC DA server ProgID, used when `--server` is omitted. Unlike the other\nfields there is no built-in default -- if this is unset and `--server` is omitted,\nthe command errors (see [`resolve_server`]).",
      "type": [
        "string",
        "null"
      ]
    },
    "templates": {
      "description": "Overrides the default user-supplied DCS/PLC template catalog path (see\n[`templates_path_from`]). A file here is loaded on every startup in addition to the\nembedded built-in catalog (see `crate::db::open` and [`load_user_templates`]),\nattributed `TemplateOrigin::Catalog`.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "$defs": {
    "LogConfig": {
      "description": "Logging configuration keys (a `[log]` table in `bhtune.toml`), consumed by\n`crate::logging::resolve_log_settings`. Every field is optional and falls back through\nthe same `CLI flag > env var > config file > default` precedence as the rest of\n[`BhtuneConfig`].",
      "type": "object",
      "properties": {
        "dir": {
          "type": [
            "string",
            "null"
          ]
        },
        "format": {
          "type": [
            "string",
            "null"
          ]
        },
        "level": {
          "type": [
            "string",
            "null"
          ]
        },
        "rotation": {
          "type": [
            "string",
            "null"
          ]
        }
      }
    }
  }
}
```

## DCS/PLC template catalog

JSON Schema for one entry in a DCS/PLC template catalog TOML file (`bhtune_core::template::DcsTemplate`), the shape `bhtune template import` and the embedded/user catalogs both parse. See `docs/dcs-templates.md` for a worked example and contribution guidance.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "DcsTemplate",
  "description": "One DCS/PLC vendor's conventions.",
  "type": "object",
  "properties": {
    "controller_action_direct_value": {
      "description": "The raw value a Controller Direction tag holds when the controller is Direct\nacting; see [`crate::direction::ControllerDirection::from_raw_tag_value`].",
      "type": "string"
    },
    "controller_direction_suffix": {
      "type": "string"
    },
    "controller_mode_suffix": {
      "type": "string"
    },
    "derivative_constant_suffix": {
      "type": "string"
    },
    "derivative_type": {
      "$ref": "#/$defs/DerivativeType"
    },
    "derivative_unit": {
      "$ref": "#/$defs/TimeUnit"
    },
    "description": {
      "description": "Free-text description of the control system this template targets.",
      "type": [
        "string",
        "null"
      ],
      "default": null
    },
    "integral_constant_suffix": {
      "type": "string"
    },
    "integral_type": {
      "$ref": "#/$defs/IntegralType"
    },
    "integral_unit": {
      "$ref": "#/$defs/TimeUnit"
    },
    "lower_mv_range_suffix": {
      "type": "string"
    },
    "lower_pv_range_suffix": {
      "type": "string"
    },
    "manipulated_variable_suffix": {
      "type": "string"
    },
    "mode_attribute_program_value": {
      "description": "The raw value a Mode Attribute tag holds when in \"Program\" mode (permits external\nOPC writes). `None` when the DCS has no mode-attribute concept.",
      "type": [
        "string",
        "null"
      ]
    },
    "mode_attribute_suffix": {
      "type": "string"
    },
    "mode_auto_value": {
      "type": "string"
    },
    "mode_manual_value": {
      "description": "The DCS-specific raw values a Mode tag holds for Manual/Auto.",
      "type": "string"
    },
    "name": {
      "type": "string"
    },
    "process_variable_suffix": {
      "description": "OPC item-name suffixes, combined with a PV tag's path prefix by\n[`crate::tags::derive_tag`] to fill in the rest of the tag set. An empty suffix\nmeans the corresponding tag is not applicable for this DCS (e.g. some DCS families\nhave no mode-attribute concept).",
      "type": "string"
    },
    "proportional_constant_suffix": {
      "type": "string"
    },
    "proportional_type": {
      "$ref": "#/$defs/ProportionalType"
    },
    "revert_mode": {
      "description": "If true, the controller mode is switched back to its original mode (e.g.\nAuto/Cascade) after a completed MRFT test. Has no effect if the loop was already in\nManual at test start.",
      "type": "boolean"
    },
    "setpoint_variable_suffix": {
      "type": "string"
    },
    "source": {
      "description": "Citation for where this mapping came from (a manual, a field deployment).\nProvenance, not a correctness guarantee -- there is deliberately no separate\n\"verified\" trust field; everything accepted into the catalog is treated as verified,\nand real mapping errors are fixed as bugs when they surface.",
      "type": [
        "string",
        "null"
      ],
      "default": null
    },
    "upper_mv_range_suffix": {
      "type": "string"
    },
    "upper_pv_range_suffix": {
      "type": "string"
    },
    "versions": {
      "description": "DCS/PLC releases this mapping is known to apply to (e.g. `[\"R5\", \"R6\"]`), in each\nvendor's own version-naming convention rather than a normalized scheme. A newer\nrelease that changes tag conventions gets its *own* template entry with its own\n`name` and `versions` list -- never an edit to this one in place, since sites still\nrunning the older release depend on the existing mapping (see\n`docs/dcs-templates.md`). May be empty for a contribution whose version coverage\nisn't yet known; `name` is what makes a template unique, not `versions`.",
      "type": "array",
      "default": [],
      "items": {
        "type": "string"
      }
    }
  },
  "required": [
    "name",
    "revert_mode",
    "proportional_type",
    "integral_type",
    "integral_unit",
    "derivative_type",
    "derivative_unit",
    "process_variable_suffix",
    "manipulated_variable_suffix",
    "setpoint_variable_suffix",
    "controller_direction_suffix",
    "controller_mode_suffix",
    "mode_attribute_suffix",
    "upper_pv_range_suffix",
    "lower_pv_range_suffix",
    "upper_mv_range_suffix",
    "lower_mv_range_suffix",
    "proportional_constant_suffix",
    "integral_constant_suffix",
    "derivative_constant_suffix",
    "mode_manual_value",
    "mode_auto_value",
    "controller_action_direct_value"
  ],
  "$defs": {
    "DerivativeType": {
      "description": "How a DCS expresses the derivative term.",
      "oneOf": [
        {
          "description": "Td: derivative time.",
          "type": "string",
          "const": "derivative_time"
        },
        {
          "description": "Kd: derivative gain, `Kd = Kp * Td`.",
          "type": "string",
          "const": "derivative_gain"
        }
      ]
    },
    "IntegralType": {
      "description": "How a DCS expresses the integral term.",
      "oneOf": [
        {
          "description": "Ti: reset time.",
          "type": "string",
          "const": "reset_time"
        },
        {
          "description": "Ri: reset rate, `Ri = 1 / Ti`.",
          "type": "string",
          "const": "reset_rate"
        },
        {
          "description": "Ki: reset gain, `Ki = Kp / Ti`.",
          "type": "string",
          "const": "reset_gain"
        }
      ]
    },
    "ProportionalType": {
      "description": "How a DCS expresses the proportional term.",
      "oneOf": [
        {
          "description": "Kp: dimensionless gain.",
          "type": "string",
          "const": "gain"
        },
        {
          "description": "PB: proportional band, as a percentage. `PB = 100 / Kp`.",
          "type": "string",
          "const": "band"
        }
      ]
    },
    "TimeUnit": {
      "description": "The time unit a DCS expects for integral/derivative parameters.",
      "type": "string",
      "enum": [
        "seconds",
        "minutes"
      ]
    }
  }
}
```
