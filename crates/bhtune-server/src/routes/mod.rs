//! HTTP route modules, one per resource. Each exposes a plain `pub fn router() ->
//! axum::Router<AppState>`, merged together in [`crate::build_router`].

pub mod health;
pub mod history;
pub mod opc;
pub mod runs;
pub mod stream;
pub mod templates;
