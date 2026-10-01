use super::{ApiError, SearchIndexControlAction, SearchMatchMode};

pub(super) fn default_index_search_max_results() -> u32 {
    bhtune_driver::DEFAULT_INDEX_SEARCH_MAX_RESULTS
}

pub(super) fn parse_search_index_control_action(
    value: &str,
) -> Result<SearchIndexControlAction, ApiError> {
    match value {
        "pause" => Ok(SearchIndexControlAction::Pause),
        "resume" => Ok(SearchIndexControlAction::Resume),
        "cancel" => Ok(SearchIndexControlAction::Cancel),
        _ => Err(ApiError::BadRequest(
            "action must be one of: pause, resume, cancel".to_string(),
        )),
    }
}

pub(super) fn default_page_size() -> u32 {
    bhtune_driver::DEFAULT_PAGE_SIZE
}

pub(super) fn validate_positive(value: u32, field: &str) -> Result<u32, ApiError> {
    if value == 0 {
        return Err(ApiError::BadRequest(format!(
            "{field} must be greater than zero"
        )));
    }
    Ok(value)
}

pub(super) fn default_search_match_mode() -> String {
    "contains".to_string()
}

pub(super) fn default_search_max_results() -> u32 {
    bhtune_driver::DEFAULT_SEARCH_MAX_RESULTS
}

pub(super) fn parse_search_match_mode(value: &str) -> Result<SearchMatchMode, ApiError> {
    match value {
        "exact" => Ok(SearchMatchMode::Exact),
        "prefix" => Ok(SearchMatchMode::Prefix),
        "contains" => Ok(SearchMatchMode::Contains),
        _ => Err(ApiError::BadRequest(
            "match_mode must be one of: exact, prefix, contains".to_string(),
        )),
    }
}
