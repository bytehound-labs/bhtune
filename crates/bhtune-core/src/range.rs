//! Validated PV/MV range types -- the boundary a live OPC DA read or a CLI flag override
//! must pass through before an untrusted number is treated as a range with a known,
//! trustworthy shape (finite bounds, correctly ordered, non-zero span). See AGENTS.md's
//! "Live-plant safety hardening" section for the review finding this closes (`--cycles-count
//! 0` panicking mid-run was the same finding's other symptom: no externally supplied number
//! reached the engine validated).
//!
//! [`PvRange`] and [`MvRange`] both still expose plain public fields and can be constructed
//! directly with a struct literal -- deliberately, since that's how already-trusted values
//! (test fixtures, values reloaded from `bhtune-db` that were already validated once before
//! being stored) are constructed elsewhere in the codebase. [`PvRange::new`]/[`MvRange::new`]
//! are the *validating* constructors: the ones any caller reading a number from a live
//! backend or an external CLI flag/config value must go through.

use serde::{Deserialize, Serialize};

/// PV scale range, read once before the test starts (`PvSH`/`PvSL` in the legacy app's
/// `ReadInitialOPCvalues`) -- distinct from [`MvRange`], used only to express the
/// oscillation amplitude as a percentage in `core-tuning-math::measure_oscillation`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PvRange {
    pub high: f32,
    pub low: f32,
}

impl PvRange {
    /// Validates a PV range read from an untrusted source (a live OPC DA tag, a CLI flag
    /// override). Rejects a non-finite bound (NaN/infinite) or a zero span (`high == low`,
    /// which would make `measure_oscillation`'s amplitude-as-a-percentage-of-range
    /// calculation divide by zero). Unlike [`MvRange`], the two bounds are not required to
    /// be in `low < high` order -- only genuinely distinct -- since the PV range is used
    /// solely as a span magnitude here, not as an inequality bound the way the MV range is
    /// in `clamp_relay_amplitude`.
    pub fn new(high: f32, low: f32) -> Result<Self, RangeError> {
        if !high.is_finite() {
            return Err(RangeError::NotFinite {
                field: "pv_range_high",
                value: high,
            });
        }
        if !low.is_finite() {
            return Err(RangeError::NotFinite {
                field: "pv_range_low",
                value: low,
            });
        }
        if high == low {
            return Err(RangeError::ZeroSpan { high, low });
        }
        Ok(PvRange { high, low })
    }
}

/// MV range floor/ceiling (`MvMSL`/`MvMSH` in the legacy app) -- the bounds
/// [`crate::mrft::clamp_relay_amplitude`] clamps the relay step within, and the range an
/// initial MV reading must fall inside before a test can safely begin stroking it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MvRange {
    pub high: f32,
    pub low: f32,
}

impl MvRange {
    /// Validates an MV range read from an untrusted source. Rejects a non-finite bound, and
    /// (unlike [`PvRange`]) requires strict `low < high` ordering --
    /// `clamp_relay_amplitude`'s boundary-clamping comparisons (`mv_ini + amp >
    /// mv_range_high`, `mv_ini - amp < mv_range_low`) assume that orientation, and
    /// `high - low` is used directly as a positive span multiplier for the relay amplitude.
    pub fn new(high: f32, low: f32) -> Result<Self, RangeError> {
        if !high.is_finite() {
            return Err(RangeError::NotFinite {
                field: "mv_range_high",
                value: high,
            });
        }
        if !low.is_finite() {
            return Err(RangeError::NotFinite {
                field: "mv_range_low",
                value: low,
            });
        }
        if low >= high {
            return Err(RangeError::LowNotBelowHigh { low, high });
        }
        Ok(MvRange { high, low })
    }

    /// Whether `value` falls within `[low, high]` (inclusive) -- used to check an initial MV
    /// reading actually lies inside its own reported range before a test starts stroking it.
    pub fn contains(&self, value: f32) -> bool {
        value >= self.low && value <= self.high
    }
}

/// Why [`PvRange::new`]/[`MvRange::new`] rejected a range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RangeError {
    /// One bound was NaN or infinite.
    NotFinite { field: &'static str, value: f32 },
    /// [`PvRange`]'s two bounds were exactly equal (a zero-width range).
    ZeroSpan { high: f32, low: f32 },
    /// [`MvRange`]'s low bound was not strictly below its high bound.
    LowNotBelowHigh { low: f32, high: f32 },
}

impl std::fmt::Display for RangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RangeError::NotFinite { field, value } => {
                write!(f, "{field} value {value} is not a finite number")
            }
            RangeError::ZeroSpan { high, low } => write!(
                f,
                "range has zero span: high ({high}) and low ({low}) must not be equal"
            ),
            RangeError::LowNotBelowHigh { low, high } => write!(
                f,
                "range low ({low}) must be strictly less than high ({high})"
            ),
        }
    }
}

impl std::error::Error for RangeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pv_range_accepts_a_typical_range() {
        assert!(PvRange::new(100.0, 0.0).is_ok());
    }

    #[test]
    fn pv_range_accepts_a_descending_range() {
        // Only distinctness is required for PvRange, not a particular order.
        assert!(PvRange::new(0.0, 100.0).is_ok());
    }

    #[test]
    fn pv_range_rejects_zero_span() {
        assert_eq!(
            PvRange::new(50.0, 50.0),
            Err(RangeError::ZeroSpan {
                high: 50.0,
                low: 50.0
            })
        );
    }

    #[test]
    fn pv_range_rejects_non_finite_high() {
        assert!(matches!(
            PvRange::new(f32::NAN, 0.0),
            Err(RangeError::NotFinite {
                field: "pv_range_high",
                ..
            })
        ));
    }

    #[test]
    fn pv_range_rejects_non_finite_low() {
        assert!(matches!(
            PvRange::new(100.0, f32::INFINITY),
            Err(RangeError::NotFinite {
                field: "pv_range_low",
                ..
            })
        ));
    }

    #[test]
    fn mv_range_accepts_a_typical_range() {
        assert!(MvRange::new(100.0, 0.0).is_ok());
    }

    #[test]
    fn mv_range_rejects_equal_bounds() {
        assert!(matches!(
            MvRange::new(50.0, 50.0),
            Err(RangeError::LowNotBelowHigh { .. })
        ));
    }

    #[test]
    fn mv_range_rejects_descending_bounds() {
        assert!(matches!(
            MvRange::new(0.0, 100.0),
            Err(RangeError::LowNotBelowHigh { .. })
        ));
    }

    #[test]
    fn mv_range_rejects_non_finite_high() {
        assert!(matches!(
            MvRange::new(f32::NAN, 0.0),
            Err(RangeError::NotFinite {
                field: "mv_range_high",
                ..
            })
        ));
    }

    #[test]
    fn mv_range_rejects_non_finite_low() {
        assert!(matches!(
            MvRange::new(100.0, f32::NAN),
            Err(RangeError::NotFinite {
                field: "mv_range_low",
                ..
            })
        ));
    }

    #[test]
    fn mv_range_contains_checks_inclusive_bounds() {
        let range = MvRange::new(100.0, 0.0).unwrap();
        assert!(range.contains(0.0));
        assert!(range.contains(100.0));
        assert!(range.contains(50.0));
        assert!(!range.contains(-0.01));
        assert!(!range.contains(100.01));
    }

    #[test]
    fn range_error_is_a_std_error() {
        let err = RangeError::ZeroSpan {
            high: 1.0,
            low: 1.0,
        };
        let _: Box<dyn std::error::Error> = Box::new(err);
    }

    #[test]
    fn range_error_display_names_the_field() {
        let err = RangeError::NotFinite {
            field: "pv_range_high",
            value: f32::NAN,
        };
        assert!(err.to_string().contains("pv_range_high"));
    }

    #[test]
    fn serde_round_trip() {
        let range = PvRange {
            high: 100.0,
            low: 0.0,
        };
        let json = serde_json::to_string(&range).unwrap();
        let back: PvRange = serde_json::from_str(&json).unwrap();
        assert_eq!(range, back);

        let range = MvRange {
            high: 100.0,
            low: 0.0,
        };
        let json = serde_json::to_string(&range).unwrap();
        let back: MvRange = serde_json::from_str(&json).unwrap();
        assert_eq!(range, back);
    }
}
