use super::*;
use serde::Serialize;

pub(super) struct DemoIdentity {
    pub(super) token_hash: String,
    pub(super) persisted: Option<DemoSessionRow>,
}

#[derive(Serialize)]
pub(super) struct DemoStreamDone {
    pub(super) outcome: TuneOutcome,
}
