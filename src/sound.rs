//! The alarm's loud beeping sound, played through the server's default audio output.
//!
//! We synthesize the beeps in code instead of shipping a sound file: a beep is just a
//! square wave switched on and off, so there's no file to install or lose.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::system_volume;
use rodio::source::{SquareWave, Zero};
use rodio::{ChannelCount, DeviceSinkBuilder, Player, SampleRate, Source};

/// How long the alarm keeps beeping before giving up.
const RING_FOR: Duration = Duration::from_secs(60);
/// 880 Hz is a high, piercing pitch, similar to a classic digital alarm clock.
const PITCH_HZ: f32 = 880.0;
/// 1.0 is the loudest a sample can be without distorting; leave a little headroom.
/// The final loudness also depends on the system volume (e.g. `pactl set-sink-volume`).
const MAX_VOLUME: f32 = 0.8;
/// Where the volume starts, like a volume slider at 40%, rising to 100% by `RING_FOR`.
const START_SLIDER: f32 = 0.4;
const BEEP: Duration = Duration::from_millis(150);
const GAP_BETWEEN_BEEPS: Duration = Duration::from_millis(100);
const PAUSE_AFTER_GROUP: Duration = Duration::from_millis(600);
const BEEPS_PER_GROUP: u32 = 4;

// `SquareWave` always produces mono audio at 48 kHz, so silence must match it.
// These types are `NonZero` integers; `.unwrap()` is fine here because 1 and 48000
// are obviously not zero, and the compiler checks it at compile time in a `const`.
const MONO: ChannelCount = ChannelCount::new(1).unwrap();
const SAMPLE_RATE: SampleRate = SampleRate::new(48_000).unwrap();

/// A handle to an alarm that is currently ringing (or recently was).
///
/// Holding it keeps the sound going; call `stop` (or just drop it) to silence the alarm.
pub struct Ringing {
    stop_tx: Sender<()>,
}

impl Ringing {
    /// Silence the alarm. This takes `self` by value, so the handle is used up and can't
    /// be stopped twice.
    pub fn stop(self) {
        // If the sound already finished on its own, nobody is listening; that's fine.
        let _ = self.stop_tx.send(());
    }
}

/// Start beeping on a separate thread and return immediately.
///
/// The daemon must keep running its loop while the alarm sounds (to notice
/// `alarm stop`, the next alarm, etc.), so the sound gets its own thread.
pub fn ring_in_background() -> Ringing {
    // Same channel idea as the daemon's file watcher: the daemon keeps `stop_tx`
    // (inside `Ringing`) and the sound thread waits on `stop_rx`.
    let (stop_tx, stop_rx) = mpsc::channel();
    thread::spawn(move || {
        // A spawned thread has nobody to return an error to, so we log it here.
        if let Err(err) = ring(stop_rx) {
            eprintln!("Could not play the alarm sound: {err:#}");
        }
    });
    Ringing { stop_tx }
}

/// Play the beep pattern until `RING_FOR` passes or a stop message arrives.
fn ring(stop_rx: Receiver<()>) -> Result<()> {
    // `sink` is our connection to the sound card. Audio stops when it is dropped, so it
    // must stay alive (in scope) until we're done waiting at the end of this function.
    let mut sink = DeviceSinkBuilder::open_default_sink()
        .context("failed to open the default audio output")?;
    // Dropping the sink is exactly how we stop the sound, so skip rodio's warning about it.
    sink.log_on_drop(false);
    let player = Player::connect_new(sink.mixer());

    // Max out the system volume so a turned-down or muted laptop still wakes you up.
    // If that fails (e.g. wpctl is missing) the alarm should still ring, just at the
    // current volume, so we log the error and carry on. The `match` turns the `Result`
    // into an `Option`: `Some` only when there is something to restore later.
    let saved_volume = match system_volume::max_out() {
        Ok(saved) => Some(saved),
        Err(err) => {
            eprintln!("Could not turn up the system volume: {err:#}");
            None
        }
    };

    // Queue up the whole pattern: beep-beep-beep-beep ... pause ... repeat.
    let mut queued = Duration::ZERO;
    while queued < RING_FOR {
        for _ in 0..BEEPS_PER_GROUP {
            // `queued` is how far into the alarm this beep will play.
            player.append(beep(volume_at(queued)));
            player.append(silence(GAP_BETWEEN_BEEPS));
            queued += BEEP + GAP_BETWEEN_BEEPS;
        }
        player.append(silence(PAUSE_AFTER_GROUP));
        queued += PAUSE_AFTER_GROUP;
    }

    // Wait (using no CPU) for whichever comes first: a stop message, the `Ringing`
    // handle being dropped (`Disconnected`), or `RING_FOR` running out. We don't care
    // which one it was — in every case we return, which drops `player` and `sink`
    // and cuts the sound off mid-beep.
    let _ = stop_rx.recv_timeout(RING_FOR);

    // Silence the speakers *before* restoring the volume, so the last beep doesn't
    // suddenly get quieter. `drop` ends a value's life early instead of waiting for `}`.
    drop(player);
    drop(sink);
    // `if let Some(..)` skips this if maxing out the volume failed earlier.
    if let Some(saved) = saved_volume
        && let Err(err) = saved.restore()
    {
        eprintln!("Could not restore the system volume: {err:#}");
    }
    Ok(())
}

/// One short, loud beep.
fn beep(volume: f32) -> impl Source {
    SquareWave::new(PITCH_HZ)
        .amplify(volume)
        .take_duration(BEEP)
}

/// How loud a beep should be when it starts `elapsed` into the alarm.
///
/// We imagine sliding a volume slider from `START_SLIDER` up to 100%, then convert the
/// slider position to a signal level by cubing it — the same curve PipeWire/GNOME volume
/// sliders use. Ears hear loudness roughly logarithmically, so a plain straight line on
/// the signal (0.4 → 1.0) only sounds slightly louder; the cubed version (0.064 → 1.0)
/// sounds like turning the volume knob up for real.
fn volume_at(elapsed: Duration) -> f32 {
    // `progress` goes from 0.0 (just started) to 1.0 (RING_FOR has passed). `.min(1.0)`
    // keeps it from overshooting, since the last group of beeps can run slightly past.
    let progress = (elapsed.as_secs_f32() / RING_FOR.as_secs_f32()).min(1.0);
    let slider = START_SLIDER + (1.0 - START_SLIDER) * progress;
    MAX_VOLUME * slider.powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Floats pick up tiny rounding errors, so compare "close enough" instead of `==`.
    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.0001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn volume_ramps_like_a_slider_from_40_to_100_percent() {
        // Slider positions 40%, 70%, 100%, cubed: 0.064, 0.343, 1.0.
        assert_close(volume_at(Duration::ZERO), MAX_VOLUME * 0.064);
        assert_close(volume_at(RING_FOR / 2), MAX_VOLUME * 0.343);
        assert_close(volume_at(RING_FOR), MAX_VOLUME);
    }

    #[test]
    fn volume_never_exceeds_max() {
        assert_close(volume_at(RING_FOR * 2), MAX_VOLUME);
    }
}

fn silence(length: Duration) -> impl Source {
    Zero::new(MONO, SAMPLE_RATE).take_duration(length)
}
