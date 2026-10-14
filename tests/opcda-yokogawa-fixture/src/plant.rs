use std::{
    fs::File,
    io::{self, Write},
    time::Duration,
};

use bhtune_driver::{FopdtConfig, FopdtProcess};
use serde::Serialize;

pub const PREFIX: &str = "CI.Yokogawa.TC001";
#[cfg(windows)]
pub const BRANCHES: [&str; 3] = ["SmokeTest", "FCS_TEST", "TC001"];
pub const TAGS: [&str; 12] = [
    "PV", "MV", "SV", "DR", "MODE", "SH", "SL", "MSH", "MSL", "P", "I", "D",
];
const STEP: Duration = Duration::from_millis(20);
const MAX_WRITES: u64 = 256;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value {
    Number(f32),
    Text(String),
}

#[derive(Debug)]
pub enum WriteError {
    BadType,
    ReadOnly,
    InvalidValue,
    ManualRequired,
    Audit(io::Error),
}

pub fn tag_index(item_id: &str) -> Option<usize> {
    let suffix = item_id.strip_prefix(PREFIX)?.strip_prefix('.')?;
    TAGS.iter().position(|tag| *tag == suffix)
}

pub fn writable(index: usize) -> bool {
    matches!(index, 1 | 2 | 4 | 9..=11)
}

pub struct Plant {
    model: FopdtProcess,
    advanced: Duration,
    manual: bool,
    setpoint: f32,
    pid: [f32; 3],
    audit: File,
    sequence: u64,
}

impl Plant {
    pub fn new(audit: File) -> Self {
        Self {
            model: FopdtProcess::new(FopdtConfig::new(1.0, 2.0, 1.0, 0.02), 50.0, 50.0, 0),
            advanced: Duration::ZERO,
            manual: false,
            setpoint: 50.0,
            pid: [120.0, 30.0, 5.0],
            audit,
            sequence: 0,
        }
    }

    pub fn advance(&mut self, elapsed: Duration) {
        while self.advanced + STEP <= elapsed {
            self.model.step();
            self.advanced += STEP;
        }
    }

    pub fn mode(&self) -> &'static str {
        if self.manual { "MAN" } else { "AUT" }
    }

    pub fn read(&self, index: usize) -> Option<Value> {
        let number = match index {
            0 => self.model.pv(),
            1 => self.model.mv(),
            2 => self.setpoint,
            3 => 1.0,
            4 => return Some(Value::Text(self.mode().to_owned())),
            5 | 7 => 100.0,
            6 | 8 => 0.0,
            9..=11 => self.pid[index - 9],
            _ => return None,
        };
        Some(Value::Number(number))
    }

    pub fn write(
        &mut self,
        index: usize,
        value: Value,
        elapsed: Duration,
    ) -> Result<(), WriteError> {
        if !writable(index) {
            return Err(WriteError::ReadOnly);
        }
        let value = if index == 4 {
            match value {
                Value::Text(ref text) if text == "MAN" || text == "AUT" => value,
                Value::Text(_) => return Err(WriteError::InvalidValue),
                Value::Number(_) => return Err(WriteError::BadType),
            }
        } else {
            // OPC DA converts numeric BSTR writes to the canonical type; restoration uses them.
            let number = match value {
                Value::Number(number) => number,
                Value::Text(text) => text.parse().map_err(|_| WriteError::BadType)?,
            };
            if !number.is_finite()
                || (matches!(index, 1 | 2) && !(0.0..=100.0).contains(&number))
                || (index >= 9 && number <= 0.0)
            {
                return Err(WriteError::InvalidValue);
            }
            if index == 1 && !self.manual {
                return Err(WriteError::ManualRequired);
            }
            Value::Number(number)
        };
        if self.sequence >= MAX_WRITES {
            return Err(WriteError::Audit(io::Error::other(
                "fixture write audit limit exceeded",
            )));
        }
        self.advance(elapsed);
        let record = serde_json::json!({
            "sequence": self.sequence + 1,
            "tag": format!("{PREFIX}.{}", TAGS[index]),
            "value": value,
            "mode_before": self.mode(),
        });
        serde_json::to_writer(&mut self.audit, &record)
            .map_err(io::Error::other)
            .map_err(WriteError::Audit)?;
        self.audit.write_all(b"\n").map_err(WriteError::Audit)?;
        self.audit.flush().map_err(WriteError::Audit)?;
        self.sequence += 1;
        match (index, value) {
            (1, Value::Number(number)) => self.model.set_mv(number),
            (2, Value::Number(number)) => self.setpoint = number,
            (4, Value::Text(text)) => self.manual = text == "MAN",
            (9..=11, Value::Number(number)) => self.pid[index - 9] = number,
            _ => return Err(WriteError::BadType),
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Fixture assertions fail the test, not a live controller operation"
)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_time_does_not_depend_on_read_count_and_old_input_advances_before_write() {
        let mut plant = Plant::new(tempfile::tempfile().unwrap());
        plant
            .write(4, Value::Text("MAN".into()), Duration::ZERO)
            .unwrap();
        plant
            .write(1, Value::Number(70.0), Duration::from_secs(1))
            .unwrap();
        assert_eq!(plant.read(0), Some(Value::Number(50.0)));
        plant.advance(Duration::from_secs(4));
        let pv = plant.read(0).unwrap();
        assert!(matches!(pv, Value::Number(value) if value > 50.0 && value < 70.0));
        for _ in 0..100 {
            plant.advance(Duration::from_secs(4));
            assert_eq!(plant.read(0), Some(pv.clone()));
        }
    }

    #[test]
    fn invalid_tags_types_and_writes_fail_without_changing_state_or_audit() {
        let mut plant = Plant::new(tempfile::tempfile().unwrap());
        assert_eq!(tag_index("CI.Yokogawa.TC001.PV"), Some(0));
        assert_eq!(tag_index("CI.Yokogawa.TC001.PV.extra"), None);
        assert_eq!(tag_index("other.PV"), None);
        for (index, value) in [
            (0, Value::Number(12.0)),
            (1, Value::Number(70.0)),
            (4, Value::Number(1.0)),
            (4, Value::Text("wrong".into())),
            (9, Value::Number(f32::NAN)),
            (9, Value::Number(0.0)),
            (10, Value::Text("not a number".into())),
        ] {
            assert!(plant.write(index, value, Duration::ZERO).is_err());
        }
        assert_eq!(plant.read(1), Some(Value::Number(50.0)));
        assert_eq!(plant.read(9), Some(Value::Number(120.0)));
        assert_eq!(plant.mode(), "AUT");
        assert_eq!(plant.audit.metadata().unwrap().len(), 0);
        plant
            .write(2, Value::Text("50".into()), Duration::ZERO)
            .unwrap();
        assert_eq!(plant.read(2), Some(Value::Number(50.0)));
        assert_eq!(plant.sequence, 1);
        for _ in 1..MAX_WRITES {
            plant.write(2, Value::Number(50.0), Duration::ZERO).unwrap();
        }
        let audit_length = plant.audit.metadata().unwrap().len();
        assert!(matches!(
            plant.write(2, Value::Number(60.0), Duration::ZERO),
            Err(WriteError::Audit(error)) if error.kind() == io::ErrorKind::Other
        ));
        assert_eq!(plant.read(2), Some(Value::Number(50.0)));
        assert_eq!(plant.audit.metadata().unwrap().len(), audit_length);
    }
}
