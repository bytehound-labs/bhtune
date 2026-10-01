use bhtune_core::{ControllerDirection, ControllerType, ProcessType, ResponseLevel, TagOverrides};
use bhtune_db::models::TuneDriver;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

fn default_sim_gain() -> f32 {
    1.0
}
fn default_sim_tau() -> f32 {
    2.0
}
fn default_sim_dead_time() -> f32 {
    5.0
}
fn default_sim_initial_value() -> f32 {
    50.0
}

/// The body of `POST /api/runs` contains the per-run tune inputs. Operational timing values
/// are intentionally absent: they are resolved from the global `[tuning]` configuration by
/// `prepare()`, just as they are for a CLI invocation. Every field that has a CLI default
/// (`--sim-gain`, etc.) repeats that exact default here via `#[serde(default = "...")]`, so an
/// HTTP caller that omits a field gets identical behavior to a CLI invocation that omits the
/// matching flag. `Option<T>` fields need no `#[serde(default)]` of their own -- serde already
/// treats a missing key as `None` for an `Option` field.
///
/// Also derives `Serialize` so the exact same type can serve as `GET /api/runs/last-request`'s
/// response (`ui-prefill-last-run`, in `routes::history::last_request`): that endpoint parses
/// a run's stored `request_json` straight into a `StartRunRequest` rather than duplicating
/// its ~30 fields into a second struct, giving a "what you `GET` is exactly what you'd `POST`
/// to repeat it" symmetry in both the Rust types and the generated OpenAPI schema. This is
/// safe precisely because `request_json` is *already* built to this exact shape -- the
/// runtime's `RequestSnapshot` serializes the same transport-neutral request fields.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StartRunRequest {
    /// PV tag prefix; ignored for `driver: "simulator"`.
    pub tagname: String,
    /// DCS/PLC template name (see `GET /api/templates`).
    pub template: String,
    pub process_type: ProcessType,
    pub controller_type: ControllerType,
    /// Relay amplitude, as a percentage of the MV range.
    pub relay_amp: f32,
    /// Relay cycles to skip before counting begins (default: looked up per `process_type`).
    pub cycles_skip: Option<u32>,
    /// Relay cycles to count once the skip period ends (default: looked up per
    /// `process_type`).
    pub cycles_count: Option<u32>,
    /// Seconds a switch must persist before it's accepted (default: looked up per
    /// `process_type`).
    pub noise_protection_secs: Option<u32>,
    /// Which driver drives this tune. `"replay"` is rejected -- that driver exists only
    /// for offline golden-trace validation, not for starting a live/simulated run.
    pub driver: TuneDriver,
    /// opcda-bridge gateway address. Only meaningful with `driver: "opcda"` (default:
    /// resolved the same way the CLI resolves `--bridge-host`, via this process's own
    /// config/env).
    pub bridge_host: Option<String>,
    /// OPC DA server ProgID. Required with `driver: "opcda"`.
    pub server: Option<String>,
    /// Simulator process gain (`driver: "simulator"` only).
    #[serde(default = "default_sim_gain")]
    pub sim_gain: f32,
    /// Simulator process time constant, in seconds (`driver: "simulator"` only).
    #[serde(default = "default_sim_tau")]
    pub sim_tau: f32,
    /// Simulator dead time, in seconds (`driver: "simulator"` only).
    #[serde(default = "default_sim_dead_time")]
    pub sim_dead_time: f32,
    /// Simulator measurement noise amplitude (`driver: "simulator"` only).
    #[serde(default)]
    pub sim_noise: f32,
    /// Simulator RNG seed, for reproducible noise (`driver: "simulator"` only).
    #[serde(default)]
    pub sim_seed: u64,
    /// Simulator initial PV (`driver: "simulator"` only).
    #[serde(default = "default_sim_initial_value")]
    pub sim_initial_pv: f32,
    /// Simulator initial MV (`driver: "simulator"` only).
    #[serde(default = "default_sim_initial_value")]
    pub sim_initial_mv: f32,
    /// Fixed PV range high, overriding a live tag read. Required for `driver: "simulator"`,
    /// which has no range tags at all.
    pub pv_range_high: Option<f32>,
    /// Fixed PV range low, overriding a live tag read.
    pub pv_range_low: Option<f32>,
    /// Fixed MV range high, overriding a live tag read.
    pub mv_range_high: Option<f32>,
    /// Fixed MV range low, overriding a live tag read.
    pub mv_range_low: Option<f32>,
    /// Fixed controller direction, overriding a live tag read.
    pub direction: Option<ControllerDirection>,
    /// Per-tune replacements for template-derived OPC tag names. Blank or missing fields use
    /// the template-derived tag.
    pub tag_overrides: Option<TagOverrides>,
    /// Operator notes to attach to this run. Notes can be edited or cleared later through
    /// the run-history endpoints.
    #[serde(default)]
    pub notes: Option<String>,
    /// Confirm an unattended PID write-back. Required alongside `write_pid` -- the request
    /// is rejected otherwise, identically to `--write-pid` without `--yes` on the CLI.
    #[serde(default)]
    pub yes: bool,
    /// Non-interactively write this response level's calculated PID parameters back to the
    /// DCS. Requires `yes: true`.
    pub write_pid: Option<ResponseLevel>,
}

/// The body of `PUT /api/runs/{id}/notes`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateNotesRequest {
    /// Replacement note text. Blank or whitespace-only text clears the note.
    pub notes: String,
}

/// The body of `POST /api/runs/{id}/write`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WriteRunRequest {
    /// Which of the run's three calculated candidate result sets to write.
    pub response_level: ResponseLevel,
}
