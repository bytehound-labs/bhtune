use bhtune_driver::Quality;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuneOutcome {
    Completed,
    Aborted,
    TimedOut,
    PoorQuality,
    ActuationFailed,
    WriteBackFailed,
    RestoreIncomplete,
}

impl TuneOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Aborted => "aborted",
            Self::TimedOut => "timed_out",
            Self::PoorQuality => "poor_quality",
            Self::ActuationFailed => "actuation_failed",
            Self::WriteBackFailed => "write_back_failed",
            Self::RestoreIncomplete => "restore_incomplete",
        }
    }
}

#[derive(Debug)]
pub enum RunOutcome {
    Completed {
        write_back: WriteBackOutcome,
        write_back_detail: Option<String>,
    },
    Aborted(AbortReason),
    RestoreIncomplete {
        reason: String,
    },
}

#[derive(Debug)]
pub struct TuneRunReport {
    pub run_id: i64,
    pub outcome: RunOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AbortReason {
    UserInterrupt,
    Timeout {
        timeout_secs: u64,
    },
    OperationTimedOut {
        tag: String,
        op_timeout_secs: u64,
    },
    PoorQuality {
        tag: String,
        quality: Quality,
    },
    MvActuationUnconfirmed {
        tag: String,
        target: f32,
        readback: Option<f32>,
        tolerance: f32,
        elapsed_ms: u64,
        deadline_secs: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteBackOutcome {
    Skipped,
    Written {
        response_level: bhtune_core::ResponseLevel,
    },
    Failed,
}

pub fn tune_outcome_for_run(outcome: &RunOutcome) -> TuneOutcome {
    match outcome {
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Failed,
            ..
        } => TuneOutcome::WriteBackFailed,
        RunOutcome::Completed { .. } => TuneOutcome::Completed,
        RunOutcome::Aborted(AbortReason::UserInterrupt) => TuneOutcome::Aborted,
        RunOutcome::Aborted(
            AbortReason::Timeout { .. } | AbortReason::OperationTimedOut { .. },
        ) => TuneOutcome::TimedOut,
        RunOutcome::Aborted(AbortReason::PoorQuality { .. }) => TuneOutcome::PoorQuality,
        RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. }) => {
            TuneOutcome::ActuationFailed
        }
        RunOutcome::RestoreIncomplete { .. } => TuneOutcome::RestoreIncomplete,
    }
}

pub(super) fn format_mv_actuation_abort_reason(reason: &AbortReason) -> String {
    let AbortReason::MvActuationUnconfirmed {
        tag,
        target,
        readback,
        tolerance,
        elapsed_ms,
        deadline_secs,
    } = reason
    else {
        return format!("{reason:?}");
    };
    let readback = readback
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unavailable".to_string());
    format!(
        "MV actuation unconfirmed: tag '{tag}', target {target}, readback {readback}, tolerance {tolerance}, elapsed {elapsed_ms} ms, deadline {deadline_secs} s"
    )
}

#[cfg(test)]
mod tests {
    use super::TuneOutcome;

    #[test]
    fn outcome_labels_preserve_the_cli_json_contract() {
        for (outcome, label) in [
            (TuneOutcome::Completed, "completed"),
            (TuneOutcome::Aborted, "aborted"),
            (TuneOutcome::TimedOut, "timed_out"),
            (TuneOutcome::PoorQuality, "poor_quality"),
            (TuneOutcome::ActuationFailed, "actuation_failed"),
            (TuneOutcome::WriteBackFailed, "write_back_failed"),
            (TuneOutcome::RestoreIncomplete, "restore_incomplete"),
        ] {
            assert_eq!(outcome.label(), label);
        }
    }
}
