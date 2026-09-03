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
    "allow_uncertain_quality": {
      "description": "Default OPC sample-quality policy for the server config page: `true` accepts\n`Uncertain` quality, while `false` rejects it. A missing key is treated as `true`\nwhen the config file is parsed, matching the configuration-page default rather than\n`bool`'s ordinary `false`.",
      "type": "boolean",
      "default": true
    },
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
    "demo": {
      "description": "Safety limits for the simulator-only public demo mode.",
      "$ref": "#/$defs/DemoPolicyConfig",
      "default": {
        "accepted_start_window_secs": null,
        "accepted_starts_per_client_ip": null,
        "accepted_starts_per_token": null,
        "cleanup_interval_secs": null,
        "max_active_runs_global": null,
        "max_active_runs_per_visitor": null,
        "max_json_body_bytes": null,
        "max_runs_per_session": null,
        "max_sse_global": null,
        "max_sse_per_visitor": null,
        "max_tune_run_rows_global": null,
        "ordinary_request_concurrency": null,
        "ordinary_request_timeout_secs": null,
        "poll_interval_ms": null,
        "retained_runs_per_visitor": null,
        "run_timeout_secs": null,
        "session_ttl_secs": null,
        "sse_lifetime_secs": null
      }
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
    "origin": {
      "description": "Exact browser origin allowed for state-changing HTTP requests. `BHTUNE_ORIGIN`\noverrides this value. Demo mode requires HTTPS, except for explicit loopback HTTP\norigins used by local tests and development.",
      "type": [
        "string",
        "null"
      ],
      "default": null
    },
    "retention_days": {
      "description": "Age-based history retention (`history-retention`): tune runs with `started_at` older\nthan this many days are deleted automatically on every startup (both binaries, via\n`crate::db::open`) and, for `bhtune-server`, again on a periodic timer while it keeps\nrunning -- see `crate::retention`. A present value must be at least 1. `None` (the\ndefault) means retain forever: there is no built-in number of days, since at this\nproject's data volumes (see AGENTS.md's History explorer notes) an unexpected\nauto-delete of someone's baseline tune is a worse failure mode than an ever-growing\ndatabase file. See [`resolve_retention_days`].",
      "type": [
        "integer",
        "null"
      ],
      "format": "uint32",
      "default": null,
      "minimum": 1
    },
    "server": {
      "description": "Default OPC DA server ProgID, used when `--server` is omitted. Unlike the other\nfields there is no built-in default -- if this is unset and `--server` is omitted,\nthe command errors (see [`resolve_server`]).",
      "type": [
        "string",
        "null"
      ]
    },
    "server_mode": {
      "description": "Runtime server mode. The environment variable `BHTUNE_SERVER_MODE` overrides this.",
      "anyOf": [
        {
          "$ref": "#/$defs/ServerMode"
        },
        {
          "type": "null"
        }
      ],
      "default": null
    },
    "templates": {
      "description": "Overrides the default user-supplied DCS/PLC template catalog path (see\n[`templates_path_from`]). A file here is loaded on every startup in addition to the\nembedded built-in catalog (see `crate::db::open` and [`load_user_templates`]),\nattributed `TemplateOrigin::Catalog`.",
      "type": [
        "string",
        "null"
      ]
    },
    "trusted_proxy": {
      "description": "IP address or matching-family CIDR of a reverse proxy trusted to supply the\nsingle-address `X-BHTune-Client-IP` header for Demo quota accounting.",
      "type": [
        "string",
        "null"
      ],
      "default": null
    },
    "tuning": {
      "description": "Global tune timing defaults. Missing keys remain `None` and resolve through\n[`resolve_tuning_config`] only when a tune is prepared.",
      "$ref": "#/$defs/TuningConfig",
      "default": {
        "mrft_delay_secs": null,
        "op_timeout_secs": null,
        "poll_interval_ms": null,
        "restore_timeout_secs": null,
        "timeout_secs": null
      }
    }
  },
  "$defs": {
    "DemoPolicyConfig": {
      "description": "Optional declarations of the fixed [`DemoPolicy`] contract.\n\nMissing keys receive the approved value. A present key must state that same value; public\nDemo deployments cannot weaken or silently diverge from the documented resource policy.",
      "type": "object",
      "properties": {
        "accepted_start_window_secs": {
          "description": "Window shared by both accepted-start quotas. Fixed at 600 seconds.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "maximum": 600,
          "minimum": 600
        },
        "accepted_starts_per_client_ip": {
          "description": "Accepted starts for one client IP in the quota window. Fixed at 6.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 6,
          "minimum": 6
        },
        "accepted_starts_per_token": {
          "description": "Accepted starts for one session token in the quota window. Fixed at 6.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 6,
          "minimum": 6
        },
        "cleanup_interval_secs": {
          "description": "Interval between Demo cleanup passes. Fixed at 300 seconds.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "maximum": 300,
          "minimum": 300
        },
        "max_active_runs_global": {
          "description": "Active Demo tune limit across all visitors. Fixed at 8.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 8,
          "minimum": 8
        },
        "max_active_runs_per_visitor": {
          "description": "Active Demo tune limit for one visitor. Fixed at 1.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 1,
          "minimum": 1
        },
        "max_json_body_bytes": {
          "description": "Maximum JSON request-body size. Fixed at 32,768 bytes.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "maximum": 32768,
          "minimum": 32768
        },
        "max_runs_per_session": {
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "minimum": 0
        },
        "max_sse_global": {
          "description": "Simultaneous Demo SSE streams across all visitors. Fixed at 32.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 32,
          "minimum": 32
        },
        "max_sse_per_visitor": {
          "description": "Simultaneous SSE streams for one visitor. Fixed at 2.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 2,
          "minimum": 2
        },
        "max_tune_run_rows_global": {
          "description": "Current Demo-owned `tune_runs` row limit across all visitors. Fixed at 5,000.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 5000,
          "minimum": 5000
        },
        "ordinary_request_concurrency": {
          "description": "Concurrent ordinary, non-streaming Demo API requests. Fixed at 64.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 64,
          "minimum": 64
        },
        "ordinary_request_timeout_secs": {
          "description": "Timeout for an ordinary, non-streaming Demo API request. Fixed at 10 seconds.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "maximum": 10,
          "minimum": 10
        },
        "poll_interval_ms": {
          "description": "Simulator polling interval. Fixed at 50 milliseconds.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "maximum": 50,
          "minimum": 50
        },
        "retained_runs_per_visitor": {
          "description": "Completed runs retained for one visitor. Fixed at 10.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 10,
          "minimum": 10
        },
        "run_timeout_secs": {
          "description": "Whole-run timeout. Fixed at 30 seconds.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "maximum": 30,
          "minimum": 30
        },
        "session_ttl_secs": {
          "description": "Anonymous visitor-session lifetime. Fixed at 86,400 seconds.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "maximum": 86400,
          "minimum": 86400
        },
        "sse_lifetime_secs": {
          "description": "Absolute lifetime of one Demo SSE stream. Fixed at 45 seconds.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "maximum": 45,
          "minimum": 45
        }
      },
      "additionalProperties": false
    },
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
    },
    "ServerMode": {
      "description": "Runtime server exposure mode. Full mode preserves the normal live-plant API; Demo mode\nis an explicitly restricted, simulator-only surface intended for public demonstrations.",
      "type": "string",
      "enum": [
        "full",
        "demo"
      ]
    },
    "TuningConfig": {
      "description": "Optional values authored in the `[tuning]` table.\n\nMissing keys stay `None` so callers can distinguish an explicit TOML value from a\nbuilt-in default. Use [`resolve_tuning_config`] to obtain the concrete values used by a\ntune and [`validate_tuning_config`] before preparing the run.",
      "type": "object",
      "properties": {
        "mrft_delay_secs": {
          "type": [
            "integer",
            "null"
          ],
          "format": "uint32",
          "maximum": 3600,
          "minimum": 0
        },
        "op_timeout_secs": {
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "minimum": 1
        },
        "poll_interval_ms": {
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "minimum": 1
        },
        "restore_timeout_secs": {
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "minimum": 1
        },
        "timeout_secs": {
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "minimum": 1
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
