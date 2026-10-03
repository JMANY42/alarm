# CLAUDE.md

## Who you're working with

The owner of this project is a **Rust beginner**. This project has two equal goals:

1. Build a working, always-on alarm clock that runs on their server.
2. **Teach them Rust** along the way.

## How to respond to every prompt

Every time you write or change code in this repo, you must also teach:

- **Explain the Rust concepts you used**, briefly and concretely, tied to the actual lines you
  wrote (e.g. "`Option<Alarm>` in `src/state.rs` means there may or may not be an alarm set…").
  Prefer 2–5 focused concepts per change over a long lecture.
- **Explain *why*, not just *what*.** If there was a simpler-but-worse or more-advanced option,
  say in one sentence why you didn't pick it.
- **Name the concept** so they can look it up (ownership, borrowing, `Result`, `?` operator,
  traits, derive macros, pattern matching, modules, lifetimes, etc.) and link to the relevant
  chapter of *The Rust Book* (https://doc.rust-lang.org/book/) when helpful.
- **Point out compiler errors you hit and how you fixed them** — reading compiler errors is a
  core Rust skill.
- **End with a small optional exercise** they could try themselves to reinforce the concept
  (e.g. "try adding an `alarm status --json` flag").
- Keep code idiomatic but beginner-readable: avoid clever one-liners, unnecessary generics,
  macros, or `unsafe`. Add comments where a beginner would get stuck, not on every line.
- Don't introduce a new crate without explaining what it does and why it's worth the dependency.

## Project overview

- Single binary named `alarm` (`src/main.rs`).
- `alarm daemon` — the long-running process, managed by systemd (`systemd/alarm.service`).
  Event-driven, not polling: it sleeps until the next alarm, a change to the state file
  (watched with the `notify` crate, filtered to real changes so reading the file doesn't
  wake it), or 60 seconds at most; then prints a message when the alarm time is reached.
- `alarm set-alarm HH:MM:SS [TIMEZONE]` — set the alarm (24-hour time; TIMEZONE is an IANA
  name like `America/New_York`, default `America/Chicago` since the owner lives in Dallas).
- `alarm on` / `alarm off` — enable/disable the alarm without forgetting the time.
- `alarm status` — show current settings.
- Time handling uses the `jiff` crate. Alarms are stored as wall-clock time + IANA zone name
  (never a fixed offset) so they follow daylight saving time. DST rule: a time skipped by
  spring-forward rings right after the jump; a time repeated by fall-back rings once (first
  occurrence). Keep the DST-day tests in `src/daemon.rs` passing.
- The CLI and the daemon communicate through a JSON state file at
  `~/.local/state/alarm/state.json` (override with the `ALARM_STATE_FILE` env var, used in testing).

## Commands

```sh
cargo build            # compile
cargo test             # run unit tests
cargo clippy           # lint (great for learning idiomatic Rust)
cargo install --path . # install the `alarm` binary to ~/.cargo/bin
```
