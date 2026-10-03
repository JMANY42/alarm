//! The long-running background process that watches the clock.
//!
//! Instead of polling, the daemon sleeps until whichever comes first:
//!   1. the next alarm time,
//!   2. the state file changing (`alarm set-alarm`, `alarm on/off`), or
//!   3. `MAX_SLEEP`, as a safety net against clock changes and suspend.

use std::ffi::OsStr;
use std::fs;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use notify::{RecursiveMode, Watcher};

use crate::sound;
use crate::state::State;

/// The longest we ever sleep without re-checking. A long sleep can drift if the server
/// suspends or NTP adjusts the clock, so we wake up at least this often and recompute.
const MAX_SLEEP: Duration = Duration::from_secs(60);

pub fn run() -> Result<()> {
    let path = State::path()?;
    let dir = path
        .parent()
        .context("state file path has no parent directory")?;
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;

    // A channel is a one-way pipe between threads: `tx` sends, `rx` receives.
    // The file watcher runs on its own background thread and sends `()` (an empty
    // "something happened" message) whenever the state file changes.
    let (tx, rx) = mpsc::channel();
    // The closure below runs on another thread, so it needs its own copy (`move`) of the
    // file name rather than a borrow of `path`.
    let file_name = path
        .file_name()
        .context("state file path has no file name")?
        .to_os_string();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && is_change_to(&event, &file_name)
        {
            // If the receiver is gone the daemon is shutting down, so ignore the error.
            let _ = tx.send(());
        }
    })?;
    // We watch the *directory*, not the file: `State::save` replaces the file with a
    // rename, and a watch on the old file would stop seeing changes after the first save.
    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("failed to watch {}", dir.display()))?;

    let mut state = State::load()?;
    println!("Alarm daemon started. {}", state.describe());
    warn_if_bad_timezone(&state);
    let mut previous = Timestamp::now();
    // `Some` while an alarm is ringing (or recently rang), so `alarm stop` can silence it.
    let mut ringing: Option<sound::Ringing> = None;

    loop {
        let wait = time_until_next_wake(&state, Timestamp::now());

        // Block (using no CPU) until a change message arrives or `wait` runs out.
        match rx.recv_timeout(wait) {
            Ok(()) => {
                // One save produces several file events; drain them so they count as one.
                while rx.try_recv().is_ok() {}
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("file watcher stopped unexpectedly"),
        }

        // Check with the settings we had *before* this wake-up. If we reloaded first, an
        // alarm just set for a time a few seconds in the past would ring immediately.
        let now = Timestamp::now();
        if let Some(new_ring) = check(&state, previous, now) {
            // Replacing an old handle drops it, which silences any sound still playing,
            // so two alarms never beep on top of each other.
            ringing = Some(new_ring);
        }
        previous = now;

        // Reload on every wake-up, not only on file events, so that even if the watcher
        // misses something the daemon catches up within `MAX_SLEEP`.
        match State::load() {
            Ok(new_state) => {
                // `alarm stop` writes a fresh timestamp, so a *different* value means
                // a new stop request (an old one left in the file is ignored).
                if new_state.stop_requested_at != state.stop_requested_at
                    && let Some(ring) = ringing.take()
                {
                    ring.stop();
                    println!("Alarm stopped.");
                }
                if new_state.alarm != state.alarm || new_state.enabled != state.enabled {
                    println!("Settings changed. {}", new_state.describe());
                    warn_if_bad_timezone(&new_state);
                }
                state = new_state;
            }
            Err(err) => eprintln!("Could not read alarm state: {err:#}"),
        }
    }
}

/// Is this file event a real change (create/modify/delete/rename) to the state file?
///
/// This filter matters: the watcher also reports when a file is merely *opened*. Since the
/// daemon opens the state file every time it wakes, reacting to every event would wake
/// it again, forever — a busy loop that burns a whole CPU core.
fn is_change_to(event: &notify::Event, file_name: &OsStr) -> bool {
    let kind = event.kind;
    let is_change = kind.is_create() || kind.is_modify() || kind.is_remove();
    is_change && event.paths.iter().any(|p| p.file_name() == Some(file_name))
}

/// The alarm time and timezone, if the alarm is on and set to a valid timezone.
fn active_alarm(state: &State) -> Option<(Time, TimeZone)> {
    if !state.enabled {
        return None;
    }
    let alarm = state.alarm.as_ref()?;
    let tz = TimeZone::get(&alarm.timezone).ok()?;
    Some((alarm.time, tz))
}

fn warn_if_bad_timezone(state: &State) {
    if let Some(alarm) = &state.alarm
        && TimeZone::get(&alarm.timezone).is_err()
    {
        eprintln!(
            "Bad timezone '{}' in state file; alarm will not ring",
            alarm.timezone
        );
    }
}

/// Ring the alarm if its time passed since the last wake-up, returning a handle to the
/// sound so it can be stopped.
fn check(state: &State, previous: Timestamp, now: Timestamp) -> Option<sound::Ringing> {
    let (time, tz) = active_alarm(state)?;
    if alarm_passed(time, &tz, previous, now) {
        println!(
            "⏰ ALARM! It's {} {} ({} server time). Wake up!",
            time,
            tz.iana_name().unwrap_or("?"),
            Zoned::now().strftime("%H:%M:%S %Z"),
        );
        Some(sound::ring_in_background())
    } else {
        None
    }
}

/// How long to sleep: until the next alarm, but never longer than `MAX_SLEEP`.
fn time_until_next_wake(state: &State, now: Timestamp) -> Duration {
    let Some((time, tz)) = active_alarm(state) else {
        return MAX_SLEEP;
    };
    let Some(next) = next_alarm(time, &tz, now) else {
        return MAX_SLEEP;
    };
    // `duration_until` gives a jiff `SignedDuration` (it could be negative); converting to
    // std's `Duration` fails if it is, in which case we don't sleep at all.
    let until = Duration::try_from(now.duration_until(next)).unwrap_or(Duration::ZERO);
    until.min(MAX_SLEEP)
}

/// The exact instant the alarm rings on a given calendar day.
///
/// `to_zoned` uses jiff's "compatible" rule for daylight saving time:
///  - spring forward: a time that doesn't exist (02:30) rings after the jump (03:30)
///  - fall back: a time that happens twice (01:30) rings at the first one only
fn alarm_at(day: Date, alarm: Time, tz: &TimeZone) -> Option<Timestamp> {
    let zoned = day.to_datetime(alarm).to_zoned(tz.clone()).ok()?;
    Some(zoned.timestamp())
}

/// The next time the alarm will ring strictly after `now`.
fn next_alarm(alarm: Time, tz: &TimeZone, now: Timestamp) -> Option<Timestamp> {
    let today = now.to_zoned(tz.clone()).date();
    let candidate_days = [Some(today), today.tomorrow().ok()];
    candidate_days
        .into_iter()
        .flatten()
        .filter_map(|day| alarm_at(day, alarm, tz))
        .find(|&at| at > now)
}

/// Did the alarm time occur in the window `(previous, now]`?
///
/// We check a *window* instead of `now == alarm_time` because sleeping isn't perfectly
/// precise: we might wake at 07:00:00.004 instead of 07:00:00.000, and an exact
/// comparison would miss it. Each moment belongs to exactly one window, so the alarm
/// rings exactly once.
fn alarm_passed(alarm: Time, tz: &TimeZone, previous: Timestamp, now: Timestamp) -> bool {
    let today = now.to_zoned(tz.clone()).date();

    // Also check yesterday's date, in case the window straddles midnight.
    // `.ok()` turns the `Result` into an `Option`, dropping the (practically impossible)
    // error of stepping before the minimum supported date.
    let candidate_days = [today.yesterday().ok(), Some(today)];
    for day in candidate_days.into_iter().flatten() {
        if let Some(at) = alarm_at(day, alarm, tz)
            && previous < at
            && at <= now
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Alarm;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn time(s: &str) -> Time {
        s.parse().unwrap()
    }

    fn chicago() -> TimeZone {
        TimeZone::get("America/Chicago").unwrap()
    }

    #[test]
    fn fires_when_window_contains_alarm() {
        // 07:00:00 in Chicago in October (CDT, UTC-5) is 12:00:00 UTC.
        assert!(alarm_passed(
            time("07:00:00"),
            &chicago(),
            ts("2026-10-01T11:59:59.8Z"),
            ts("2026-10-01T12:00:00.3Z"),
        ));
    }

    #[test]
    fn does_not_fire_outside_window() {
        assert!(!alarm_passed(
            time("07:00:00"),
            &chicago(),
            ts("2026-10-01T12:00:00.3Z"),
            ts("2026-10-01T12:00:00.8Z"),
        ));
    }

    #[test]
    fn follows_standard_time_in_winter() {
        // In January Chicago is CST (UTC-6), so 07:00:00 is 13:00:00 UTC.
        assert!(alarm_passed(
            time("07:00:00"),
            &chicago(),
            ts("2027-01-15T12:59:59.8Z"),
            ts("2027-01-15T13:00:00.3Z"),
        ));
    }

    #[test]
    fn fires_across_midnight() {
        assert!(alarm_passed(
            time("23:59:59"),
            &TimeZone::UTC,
            ts("2026-10-01T23:59:58.9Z"),
            ts("2026-10-02T00:00:00.1Z"),
        ));
    }

    #[test]
    fn spring_forward_gap_rings_after_the_jump() {
        // 2027-03-14: Chicago clocks jump from 02:00 CST to 03:00 CDT, so 02:30 doesn't
        // exist. It should ring at 03:30 CDT, which is 08:30 UTC.
        assert!(alarm_passed(
            time("02:30:00"),
            &chicago(),
            ts("2027-03-14T08:29:59.8Z"),
            ts("2027-03-14T08:30:00.3Z"),
        ));
    }

    #[test]
    fn fall_back_fold_rings_only_once() {
        // 2027-11-07: Chicago clocks go from 01:59 CDT back to 01:00 CST, so 01:30
        // happens twice: 06:30 UTC (CDT) and 07:30 UTC (CST). Only the first should ring.
        assert!(alarm_passed(
            time("01:30:00"),
            &chicago(),
            ts("2027-11-07T06:29:59.8Z"),
            ts("2027-11-07T06:30:00.3Z"),
        ));
        assert!(!alarm_passed(
            time("01:30:00"),
            &chicago(),
            ts("2027-11-07T07:29:59.8Z"),
            ts("2027-11-07T07:30:00.3Z"),
        ));
    }

    #[test]
    fn next_alarm_is_later_today() {
        // 06:00 CDT; a 07:00:00 alarm is one hour away, at 12:00 UTC.
        assert_eq!(
            next_alarm(time("07:00:00"), &chicago(), ts("2026-10-01T11:00:00Z")),
            Some(ts("2026-10-01T12:00:00Z")),
        );
    }

    #[test]
    fn next_alarm_is_tomorrow_once_today_has_passed() {
        // 08:00 CDT; today's 07:00:00 is gone, so the next one is tomorrow.
        assert_eq!(
            next_alarm(time("07:00:00"), &chicago(), ts("2026-10-01T13:00:00Z")),
            Some(ts("2026-10-02T12:00:00Z")),
        );
    }

    #[test]
    fn next_alarm_on_spring_forward_day() {
        // 02:30 doesn't exist on 2027-03-14, so the next alarm is 03:30 CDT (08:30 UTC).
        assert_eq!(
            next_alarm(time("02:30:00"), &chicago(), ts("2027-03-14T06:00:00Z")),
            Some(ts("2027-03-14T08:30:00Z")),
        );
    }

    #[test]
    fn sleep_is_capped() {
        let state = State {
            alarm: Some(Alarm {
                time: time("07:00:00"),
                timezone: "America/Chicago".to_string(),
            }),
            enabled: true,
            stop_requested_at: None,
        };
        // One hour before the alarm: sleep only MAX_SLEEP.
        assert_eq!(
            time_until_next_wake(&state, ts("2026-10-01T11:00:00Z")),
            MAX_SLEEP
        );
        // Ten seconds before the alarm: sleep exactly ten seconds.
        assert_eq!(
            time_until_next_wake(&state, ts("2026-10-01T11:59:50Z")),
            Duration::from_secs(10),
        );
        // Alarm off: nothing to wait for, so sleep MAX_SLEEP.
        let off = State {
            enabled: false,
            ..state
        };
        assert_eq!(
            time_until_next_wake(&off, ts("2026-10-01T11:59:50Z")),
            MAX_SLEEP
        );
    }

    #[test]
    fn only_real_changes_to_the_state_file_wake_the_daemon() {
        use notify::event::{AccessKind, CreateKind, Event, EventKind, ModifyKind, RenameMode};
        let name = OsStr::new("state.json");
        let event = |kind, path: &str| Event::new(kind).add_path(path.into());

        // Renaming the temp file over state.json (what `State::save` does): wake up.
        let rename = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path("/dir/state.json.tmp".into())
            .add_path("/dir/state.json".into());
        assert!(is_change_to(&rename, name));

        // The daemon itself opening state.json to read it: must NOT wake up.
        let open = event(EventKind::Access(AccessKind::Any), "/dir/state.json");
        assert!(!is_change_to(&open, name));

        // Some other file in the same directory: ignore it.
        let other = event(EventKind::Create(CreateKind::File), "/dir/notes.txt");
        assert!(!is_change_to(&other, name));
    }
}
