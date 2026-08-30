//! Converts `bhtune-core`'s `serde`-tagged enums to/from the plain TEXT values SQLite stores
//! them as.
//!
//! `bhtune-core` deliberately has zero non-`serde` dependencies (see its crate docs), so it
//! cannot derive `sqlx::Type` itself, and Rust's orphan rules mean `bhtune-db` cannot
//! implement a foreign trait (`sqlx::Type`) for a foreign type (e.g.
//! `bhtune_core::ProcessType`) either. Rather than defining a parallel `sqlx`-aware enum for
//! every `bhtune-core` enum (eight of them, and rising), these two functions reuse each
//! enum's existing `#[serde(rename_all = "snake_case")]` implementation as the single source
//! of truth for its wire form — the same string a `ProcessType` would serialize to over the
//! CLI's `--output json` or the web GUI's HTTP API already matches what gets stored in a
//! `TEXT` column.

use serde::{Serialize, de::DeserializeOwned};

use crate::error::{DbError, DbResult};

/// Encodes a fieldless, `serde`-tagged enum as the bare string SQLite stores it as.
///
/// # Panics
/// Panics if `T`'s `Serialize` impl doesn't produce a bare JSON string (i.e. `T` isn't a
/// fieldless enum with `#[serde(rename_all = "snake_case")]` or equivalent). Every
/// `bhtune-core` enum stored in the database satisfies this; a panic here means a new enum
/// was wired into a TEXT column without checking that assumption first.
pub fn enum_to_text<T: Serialize>(value: &T) -> String {
    enum_to_text_with(value, |value| serde_json::to_value(value))
}

fn enum_to_text_with<T, E>(
    value: &T,
    serialize: impl FnOnce(&T) -> Result<serde_json::Value, E>,
) -> String
where
    T: Serialize,
    E: std::fmt::Debug,
{
    match serialize(value).expect("enum serialization is infallible") {
        serde_json::Value::String(s) => s,
        other => panic!(
            "enum_to_text called on a type that doesn't serialize to a bare string, got: {other}"
        ),
    }
}

/// Decodes a value read from `column` back into a fieldless, `serde`-tagged enum.
///
/// Only fails if `value` doesn't match any of `T`'s variants — which the migration's `CHECK`
/// constraint on every enum-shaped column should make unreachable in practice, but the
/// database file is plain and open (see AGENTS.md), so nothing stops something else from
/// writing a row that bypasses it.
pub fn text_to_enum<T: DeserializeOwned>(column: &'static str, value: &str) -> DbResult<T> {
    serde_json::from_value(enum_text_value(value)).map_err(|_| invalid_enum_value(column, value))
}

fn enum_text_value(value: &str) -> serde_json::Value {
    serde_json::Value::String(value.to_string())
}

fn invalid_enum_value(column: &'static str, value: &str) -> DbError {
    DbError::InvalidEnumValue {
        column,
        value: value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        MvActuationKind, MvActuationStatus, RestoreStatus, RollbackState, SampleQuality,
        TemplateOrigin, TuneDriver, TuneOutcome, WriteKind,
    };
    use bhtune_core::{
        ControllerDirection, ControllerType, DerivativeType, IntegralType, ProcessType,
        ProportionalType, ResponseLevel, TimeUnit,
    };

    /// Round-trips every variant of every `bhtune-core` enum stored in the database, and
    /// pins the exact literal each one produces. These literals must stay in sync with the
    /// `CHECK (... IN (...))` lists in `migrations/0001_initial_schema.sql` — this test is
    /// the one place both are written down side by side, so a rename on either side is easy
    /// to spot and fix in the other.
    #[test]
    fn process_type_round_trips_and_matches_check_constraint() {
        let cases = [
            (ProcessType::Flow, "flow"),
            (ProcessType::PressureLine, "pressure_line"),
            (ProcessType::PressureVessel, "pressure_vessel"),
            (ProcessType::Level, "level"),
            (ProcessType::TemperatureMixing, "temperature_mixing"),
            (
                ProcessType::TemperatureHeatExchange,
                "temperature_heat_exchange",
            ),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<ProcessType>("process_type", text).unwrap(),
                variant
            );
        }
    }

    #[test]
    #[should_panic(expected = "doesn't serialize to a bare string")]
    fn enum_to_text_rejects_non_string_serialization() {
        enum_to_text(&42u8);
    }

    #[test]
    #[should_panic(expected = "enum serialization is infallible")]
    fn enum_to_text_rejects_a_serialization_failure() {
        enum_to_text_with(&ProcessType::Flow, |_| {
            Err::<serde_json::Value, _>("injected serialization failure")
        });
    }

    #[test]
    fn controller_type_round_trips_and_matches_check_constraint() {
        let cases = [
            (ControllerType::P, "p"),
            (ControllerType::Pi, "pi"),
            (ControllerType::Pid, "pid"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<ControllerType>("controller_type", text).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn controller_direction_round_trips_and_matches_check_constraint() {
        let cases = [
            (ControllerDirection::Direct, "direct"),
            (ControllerDirection::Reverse, "reverse"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<ControllerDirection>("controller_direction", text).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn response_level_round_trips_and_matches_check_constraint() {
        let cases = [
            (ResponseLevel::Aggressive, "aggressive"),
            (ResponseLevel::Moderate, "moderate"),
            (ResponseLevel::Sluggish, "sluggish"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<ResponseLevel>("response_level", text).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn proportional_type_round_trips_and_matches_check_constraint() {
        let cases = [
            (ProportionalType::Gain, "gain"),
            (ProportionalType::Band, "band"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<ProportionalType>("proportional_type", text).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn integral_type_round_trips_and_matches_check_constraint() {
        let cases = [
            (IntegralType::ResetTime, "reset_time"),
            (IntegralType::ResetRate, "reset_rate"),
            (IntegralType::ResetGain, "reset_gain"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<IntegralType>("integral_type", text).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn derivative_type_round_trips_and_matches_check_constraint() {
        let cases = [
            (DerivativeType::DerivativeTime, "derivative_time"),
            (DerivativeType::DerivativeGain, "derivative_gain"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<DerivativeType>("derivative_type", text).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn time_unit_round_trips_and_matches_check_constraint() {
        let cases = [
            (TimeUnit::Seconds, "seconds"),
            (TimeUnit::Minutes, "minutes"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant), text);
            assert_eq!(
                text_to_enum::<TimeUnit>("integral_unit", text).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn unrecognized_value_is_a_typed_error_not_a_panic() {
        let err = text_to_enum::<ProcessType>("process_type", "not_a_real_variant").unwrap_err();
        assert!(matches!(
            err,
            DbError::InvalidEnumValue {
                column: "process_type",
                value,
            } if value == "not_a_real_variant"
        ));
    }

    #[test]
    fn database_enum_types_round_trip_through_the_shared_text_codec() {
        assert_eq!(
            text_to_enum::<TuneDriver>("driver", "opcda").unwrap(),
            TuneDriver::Opcda
        );
        assert_eq!(
            text_to_enum::<TuneOutcome>("outcome", "completed").unwrap(),
            TuneOutcome::Completed
        );
        assert_eq!(
            text_to_enum::<TemplateOrigin>("origin", "builtin").unwrap(),
            TemplateOrigin::Builtin
        );
        assert_eq!(
            text_to_enum::<RestoreStatus>("restore_status", "confirmed").unwrap(),
            RestoreStatus::Confirmed
        );
        assert_eq!(
            text_to_enum::<RollbackState>("rollback_state", "succeeded").unwrap(),
            RollbackState::Succeeded
        );
        assert_eq!(
            text_to_enum::<SampleQuality>("pv_quality", "good").unwrap(),
            SampleQuality::Good
        );
        assert_eq!(
            text_to_enum::<MvActuationKind>("kind", "relay").unwrap(),
            MvActuationKind::Relay
        );
        assert_eq!(
            text_to_enum::<MvActuationStatus>("status", "confirmed").unwrap(),
            MvActuationStatus::Confirmed
        );
        assert_eq!(
            text_to_enum::<WriteKind>("kind", "write").unwrap(),
            WriteKind::Write
        );
    }
}
