//! Turning the computer's own volume all the way up while the alarm rings.
//!
//! The volume ramp in `sound.rs` is relative to the system volume, so if the laptop is
//! turned down (or muted) the alarm would be too quiet to wake anyone. We use `wpctl`,
//! PipeWire's command-line volume tool, to max it out and later put it back.

use std::process::Command;

use anyhow::{Context, Result, bail};

/// `@DEFAULT_AUDIO_SINK@` is wpctl's name for "whatever output is currently in use".
const SINK: &str = "@DEFAULT_AUDIO_SINK@";

/// The volume settings from before the alarm, so we can restore them afterwards.
#[derive(Debug, PartialEq)]
pub struct SavedVolume {
    level: f32,
    muted: bool,
}

/// Unmute and set the volume to 100%, returning the old settings.
pub fn max_out() -> Result<SavedVolume> {
    let output = wpctl(&["get-volume", SINK])?;
    let saved = parse_volume(&output)?;
    wpctl(&["set-mute", SINK, "0"])?;
    wpctl(&["set-volume", SINK, "1.0"])?;
    Ok(saved)
}

impl SavedVolume {
    /// Put the volume and mute setting back to how they were before the alarm.
    pub fn restore(&self) -> Result<()> {
        wpctl(&["set-volume", SINK, &self.level.to_string()])?;
        let mute = if self.muted { "1" } else { "0" };
        wpctl(&["set-mute", SINK, mute])?;
        Ok(())
    }
}

/// Run `wpctl` with the given arguments and return what it printed.
fn wpctl(args: &[&str]) -> Result<String> {
    let output = Command::new("wpctl")
        .args(args)
        .output()
        .context("failed to run wpctl (is PipeWire installed?)")?;
    // A program that runs but fails (non-zero exit code) is not an `Err` from `output()`,
    // so we have to check for that ourselves.
    if !output.status.success() {
        bail!(
            "wpctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse wpctl's output, which looks like `Volume: 0.22` or `Volume: 0.22 [MUTED]`.
fn parse_volume(text: &str) -> Result<SavedVolume> {
    let rest = text
        .trim()
        .strip_prefix("Volume:")
        .with_context(|| format!("unexpected wpctl output: {text:?}"))?;
    // `split_whitespace` gives us the pieces: ["0.22"] or ["0.22", "[MUTED]"].
    let mut words = rest.split_whitespace();
    let level = words
        .next()
        .context("wpctl output has no volume number")?
        .parse::<f32>()
        .context("wpctl volume is not a number")?;
    let muted = words.any(|word| word == "[MUTED]");
    Ok(SavedVolume { level, muted })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unmuted_volume() {
        assert_eq!(
            parse_volume("Volume: 0.22\n").unwrap(),
            SavedVolume {
                level: 0.22,
                muted: false
            }
        );
    }

    #[test]
    fn parses_muted_volume() {
        assert_eq!(
            parse_volume("Volume: 0.50 [MUTED]\n").unwrap(),
            SavedVolume {
                level: 0.5,
                muted: true
            }
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_volume("").is_err());
        assert!(parse_volume("Volume: loud").is_err());
    }
}
