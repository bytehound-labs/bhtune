//! [`ApiDoc`]: the single [`utoipa::OpenApi`] aggregator listing every route and schema this
//! crate serves. Kept as its own module (rather than folded into `lib.rs`) because this list
//! is the one place that must be updated whenever a route or DTO is added -- a dedicated file
//! makes that omission easy to spot in a diff.
//!
//! There is deliberately no macro or build script scanning `routes/**` for `#[utoipa::path]`
//! annotations automatically: `utoipa`'s own design is to declare the aggregate explicitly, and
//! a missing entry here fails loudly (the route exists and works, but is simply absent from
//! `/api/openapi.json` and the Scalar docs at `/api/docs`) rather than silently -- easy to
//! catch by comparing `routes::*::router()` against this list in review.

use utoipa::OpenApi;

use crate::error::ErrorBody;
use crate::routes::{health, history, runs, templates};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "BHTune API",
        description = "HTTP API for BHTune: DCS/PLC PID-loop templates, and the tune-run history recorded by the CLI and this server.",
        license(name = "AGPL-3.0-or-later"),
    ),
    paths(
        health::health,
        templates::list_templates,
        templates::get_template,
        templates::create_template,
        templates::delete_template,
        history::list_runs,
        history::show_run,
        runs::start_run,
        runs::cancel_run,
    ),
    components(schemas(
        health::Health,
        templates::TemplateResponse,
        history::RunSummaryResponse,
        history::RunListResponse,
        history::InitialReadingsResponse,
        history::SampleResponse,
        history::ResultResponse,
        history::WriteResponse,
        history::RunDetailResponse,
        runs::StartRunRequest,
        ErrorBody,
    )),
    tags(
        (name = "health", description = "Liveness probe"),
        (name = "templates", description = "DCS/PLC template catalog (built-in, community-catalog, and user-created)"),
        (name = "runs", description = "Start, cancel, and browse the history of tune runs"),
    ),
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_spec_is_openapi_3_1() {
        let spec = ApiDoc::openapi();
        let json = spec.to_json().expect("spec must serialize to JSON");
        assert!(json.contains("\"openapi\":\"3.1.0\""));
    }

    #[test]
    fn generated_spec_serializes_to_json() {
        let spec = ApiDoc::openapi();
        let json = spec.to_json().expect("spec must serialize to JSON");
        assert!(json.contains("\"title\":\"BHTune API\""));
    }
}
