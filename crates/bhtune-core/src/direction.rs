//! Controller action direction: whether increasing the manipulated variable increases or
//! decreases the process variable. Determines which way the relay steps during MRFT.

use serde::{Deserialize, Serialize};

/// Direct-acting (increasing MV increases PV) or Reverse-acting (increasing MV decreases
/// PV).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerDirection {
    Direct,
    Reverse,
}

impl ControllerDirection {
    /// Resolves a direction read live from a DCS tag. The tag's raw string value means
    /// Direct exactly when it matches the active template's
    /// `controller_action_direct_value`; any other value means Reverse.
    pub fn from_raw_tag_value(
        raw_value: &str,
        controller_action_direct_value: &str,
    ) -> ControllerDirection {
        if raw_value == controller_action_direct_value {
            ControllerDirection::Direct
        } else {
            ControllerDirection::Reverse
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_template_direct_value() {
        assert_eq!(
            ControllerDirection::from_raw_tag_value("0", "0"),
            ControllerDirection::Direct
        );
    }

    #[test]
    fn anything_else_is_reverse() {
        assert_eq!(
            ControllerDirection::from_raw_tag_value("1", "0"),
            ControllerDirection::Reverse
        );
        assert_eq!(
            ControllerDirection::from_raw_tag_value("", "0"),
            ControllerDirection::Reverse
        );
        assert_eq!(
            ControllerDirection::from_raw_tag_value("garbage", "0"),
            ControllerDirection::Reverse
        );
    }

    #[test]
    fn serde_round_trip() {
        for dir in [ControllerDirection::Direct, ControllerDirection::Reverse] {
            let json = serde_json::to_string(&dir).unwrap();
            let back: ControllerDirection = serde_json::from_str(&json).unwrap();
            assert_eq!(dir, back);
        }
    }

    #[test]
    fn serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&ControllerDirection::Reverse).unwrap(),
            "\"reverse\""
        );
    }
}
