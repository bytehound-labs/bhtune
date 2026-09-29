use bhtune_core::{LoopConfig, LoopTags};
use chrono::{DateTime, Utc};

// loops {{{1

/// One row of `loops`: a saved, named tag mapping plus default MRFT parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopRow {
    pub id: i64,
    pub name: String,
    pub dcs_template_id: i64,
    pub tags: LoopTags,
    pub config: LoopConfig,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
// }}}1
