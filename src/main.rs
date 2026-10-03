// `mod` tells the compiler that these files (src/daemon.rs, etc.) are part of this crate.
mod daemon;
mod sound;
mod state;
mod system_volume;
mod timezone;

use anyhow::Result;
use clap::{Parser, Subcommand};
use jiff::Timestamp;
use jiff::civil::Time;

use state::{Alarm, State};

/// A simple always-on alarm clock.
#[derive(Parser)]
#[command(name = "alarm", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// clap turns each enum variant into a subcommand, converting `SetAlarm` to `set-alarm`.
#[derive(Subcommand)]
enum Command {
    /// Set the alarm time, e.g. `alarm set 07:30:00 America/New_York`
    Set {
        /// 24-hour time in HH:MM:SS format
        #[arg(value_parser = parse_time)]
        time: Time,

        /// IANA timezone name, e.g. America/Chicago, America/New_York, UTC
        #[arg(default_value = timezone::DEFAULT, value_parser = timezone::validate)]
        timezone: String,
    },
    /// Enable the alarm
    On,
    /// Disable the alarm (the alarm time is kept)
    Off,
    /// Stop the current alarm from playing
    Stop,
    /// Show the current alarm settings
    Status,
    /// Run the alarm daemon (used by the systemd service)
    Daemon,
}

/// Parse a strict HH:MM:SS string. clap calls this for us and shows the error on failure.
fn parse_time(s: &str) -> Result<Time, String> {
    Time::strptime("%H:%M:%S", s)
        .map_err(|_| format!("'{s}' is not a valid 24-hour time in HH:MM:SS format"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Set { time, timezone } => {
            let mut state = State::load()?;
            state.alarm = Some(Alarm { time, timezone });
            state.save()?;
            println!("Alarm set. {}", state.describe());
        }
        Command::On => {
            let mut state = State::load()?;
            state.enabled = true;
            state.save()?;
            println!("Alarm turned ON. {}", state.describe());
        }
        Command::Off => {
            let mut state = State::load()?;
            state.enabled = false;
            state.save()?;
            println!("Alarm turned OFF.");
        }
        Command::Stop => {
            // The CLI can't reach into the daemon's process to stop the sound, so it
            // leaves a note in the state file; the daemon's file watcher sees it instantly.
            let mut state = State::load()?;
            state.stop_requested_at = Some(Timestamp::now());
            state.save()?;
            println!("Alarm sound stopped");
        }
        Command::Status => {
            let state = State::load()?;
            println!("{}", state.describe());
        }
        Command::Daemon => daemon::run()?,
    }

    Ok(())
}
