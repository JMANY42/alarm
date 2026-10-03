//! The alarm's saved settings, shared between the CLI and the daemon via a JSON file.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use jiff::Timestamp;
use jiff::civil::Time;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Alarm {
    pub time: Time,
    /// IANA timezone name like "America/Chicago".
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    /// `None` until the user runs `alarm set-alarm`.
    pub alarm: Option<Alarm>,
    pub enabled: bool,
    /// When `alarm stop` was last run. The daemon silences a ringing alarm whenever this
    /// *changes*, so the CLI never has to reset it. `#[serde(default)]` lets older state
    /// files without this field still load (as `None`).
    #[serde(default)]
    pub stop_requested_at: Option<Timestamp>,
}

// We implement `Default` by hand (instead of `#[derive(Default)]`) because the
// derived version would make `enabled` false, and we want new alarms on by default.
impl Default for State {
    fn default() -> Self {
        State {
            alarm: None,
            enabled: true,
            stop_requested_at: None,
        }
    }
}

impl State {
    /// Where the state file lives. `ALARM_STATE_FILE` overrides it (handy for testing).
    pub fn path() -> Result<PathBuf> {
        if let Ok(custom) = std::env::var("ALARM_STATE_FILE") {
            return Ok(PathBuf::from(custom));
        }
        let home = std::env::var("HOME").context("HOME environment variable is not set")?;
        Ok(PathBuf::from(home).join(".local/state/alarm/state.json"))
    }

    /// Load the state, or return the default if the file doesn't exist yet.
    pub fn load() -> Result<State> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(State::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let state = serde_json::from_str(&text)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(state)
    }

    /// Save the state. We write to a temporary file and then rename it, because a rename
    /// is atomic: the daemon will never read a half-written file.
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    }

    /// A human-readable summary, e.g. "Alarm: 07:30:00 America/Chicago (on)".
    pub fn describe(&self) -> String {
        let on_off = if self.enabled { "on" } else { "off" };
        match &self.alarm {
            Some(alarm) => format!("Alarm: {} {} ({on_off})", alarm.time, alarm.timezone),
            None => format!("No alarm time set ({on_off})"),
        }
    }
}
