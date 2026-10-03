//! Validating IANA timezone names like "America/Chicago".
//!
//! We store a *named zone* rather than a fixed offset like UTC-6, so the alarm follows
//! daylight saving time automatically. jiff looks names up in the server's own timezone
//! database (/usr/share/zoneinfo), which normal OS updates keep current.

use jiff::tz::TimeZone;

/// Dallas is in the America/Chicago zone (IANA names zones after one big city per region).
pub const DEFAULT: &str = "America/Chicago";

/// Used by clap to validate the TIMEZONE argument. Returns the canonical spelling,
/// so `america/chicago` is saved as `America/Chicago`.
pub fn validate(name: &str) -> Result<String, String> {
    if !name.contains("/") {
        return Err(format!(
            "unknown timezone '{name}' (use an IANA name like America/Chicago or America/New_York). Do not use timezone abbreviations like UTC."
        ));
    }
    let tz = TimeZone::get(name).map_err(|_| {
        format!(
            "unknown timezone '{name}' (use an IANA name like America/Chicago or America/New_York)"
        )
    })?;
    // `iana_name()` returns an `Option` because some timezones (e.g. fixed offsets)
    // have no name. Fall back to what the user typed in that case.
    Ok(tz.iana_name().unwrap_or(name).to_string())
}

// `#[cfg(test)]` means this module is only compiled when running `cargo test`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_are_accepted() {
        assert_eq!(
            validate("America/Chicago"),
            Ok("America/Chicago".to_string())
        );
        assert_eq!(
            validate("america/new_york"),
            Ok("America/New_York".to_string())
        );
    }

    #[test]
    fn invalid_names_are_rejected() {
        assert!(validate("America/Dallas").is_err());
        assert!(validate("Not/A_Zone").is_err());
    }

    #[test]
    fn names_without_slash_are_rejected() {
        assert!(validate("UTC").is_err());
        assert!(validate("America Chicago").is_err());
    }
}
