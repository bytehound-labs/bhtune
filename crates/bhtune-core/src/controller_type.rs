//! The controller structures a tuning run can target: Proportional-only, Proportional +
//! Integral, or full PID.

use serde::{Deserialize, Serialize};

use crate::process_type::ProcessType;

/// A PID controller structure. Discriminants double as the column index into the
/// tuning-constant matrices in [`crate::constants`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ControllerType {
    P = 0,
    Pi = 1,
    Pid = 2,
}

impl ControllerType {
    /// All variants, in matrix-column order.
    pub const ALL: [ControllerType; 3] =
        [ControllerType::P, ControllerType::Pi, ControllerType::Pid];

    /// Column index into the tuning-constant matrices in [`crate::constants`].
    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// Full PID is only offered for process types where [`ProcessType::allows_pid`] is
    /// true; P and PI are available for every process type.
    pub fn is_allowed_for(self, process_type: ProcessType) -> bool {
        match self {
            ControllerType::P | ControllerType::Pi => true,
            ControllerType::Pid => process_type.allows_pid(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_matches_discriminant() {
        assert_eq!(ControllerType::P.index(), 0);
        assert_eq!(ControllerType::Pi.index(), 1);
        assert_eq!(ControllerType::Pid.index(), 2);
    }

    #[test]
    fn p_and_pi_allowed_for_every_process_type() {
        for pt in ProcessType::ALL {
            assert!(ControllerType::P.is_allowed_for(pt));
            assert!(ControllerType::Pi.is_allowed_for(pt));
        }
    }

    #[test]
    fn pid_only_allowed_for_temperature_process_types() {
        for pt in ProcessType::ALL {
            assert_eq!(
                ControllerType::Pid.is_allowed_for(pt),
                pt.allows_pid(),
                "{pt:?}"
            );
        }
    }

    #[test]
    fn serde_round_trip() {
        for ct in ControllerType::ALL {
            let json = serde_json::to_string(&ct).unwrap();
            let back: ControllerType = serde_json::from_str(&json).unwrap();
            assert_eq!(ct, back);
        }
    }

    #[test]
    fn serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&ControllerType::Pid).unwrap(),
            "\"pid\""
        );
    }
}
