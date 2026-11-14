//! What the palette's "info" panel says: this build's repo (from Cargo.toml),
//! its version, commit and when it was built (stamped by the full build's CI:
//! `TM_VERSION`, `TM_COMMIT`, `TM_BUILT`), and where it runs on this PC.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeDelta, Utc, Weekday};

const NOT_FROM_CI: &str = "a development build, not from CI";
const NO_CHECKOUT: &str = "no checkout on this PC";

/// This build's version, wherever tuimail says it: the release CI numbered it
/// as (v0.2.5.4), or Cargo.toml's with `-dev` for a build not from CI.
pub const VERSION: &str = match option_env!("TM_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

#[derive(Clone, Copy)]
struct Stamps {
    repo: &'static str,
    version: &'static str,
    commit: Option<&'static str>,
    built: Option<&'static str>,
}

const STAMPS: Stamps = Stamps {
    repo: env!("CARGO_PKG_REPOSITORY"),
    version: VERSION,
    commit: option_env!("TM_COMMIT"),
    built: option_env!("TM_BUILT"),
};

/// Read at start: once an update moves the install's `current` folder, the
/// path read again would name the new release while the old one runs.
static INSTALLED: LazyLock<String> =
    LazyLock::new(|| installed(std::env::current_exe(), |exe| std::fs::canonicalize(exe)));

/// Notes where this process runs from, before anything can move it.
pub fn read_installed_path() {
    LazyLock::force(&INSTALLED);
}

/// The panel's lines: label and value.
pub fn lines() -> Vec<(&'static str, String)> {
    lines_from(STAMPS, &INSTALLED, Utc::now(), &|path| path.try_exists())
}

fn lines_from(
    stamps: Stamps,
    installed: &str,
    now: DateTime<Utc>,
    exists: &dyn Fn(&Path) -> std::io::Result<bool>,
) -> Vec<(&'static str, String)> {
    let stamp = |s: Option<&'static str>| s.filter(|s| !s.is_empty());
    let commit = stamp(stamps.commit)
        .map_or_else(|| NOT_FROM_CI.to_string(), |c| c.chars().take(12).collect());
    // `https://host/owner/name`: the repo is what follows the host.
    let link = stamps.repo.trim_end_matches('/');
    let repo = link.splitn(4, '/').nth(3).unwrap_or(link);
    let name = repo.rsplit('/').next();
    vec![
        ("Repo", repo.to_string()),
        ("Link", link.to_string()),
        ("Version", stamps.version.to_string()),
        ("Commit", commit),
        (
            "Built",
            stamp(stamps.built).map_or_else(|| NOT_FROM_CI.to_string(), |b| built_line(b, now)),
        ),
        ("Installed", installed.to_string()),
        ("Checkout", checkout(name, exists)),
    ]
}

fn built_line(stamp: &str, now: DateTime<Utc>) -> String {
    let Ok(built) = DateTime::parse_from_rfc3339(stamp) else {
        return format!("couldn't read the build time {stamp:?}");
    };
    let built = built.to_utc();
    format!(
        "{} Pacific ({})",
        pacific(built).format("%b %-d, %-I:%M %p"),
        ago(now - built)
    )
}

/// Daylight time runs from 2 AM on March's second Sunday to 2 AM on
/// November's first.
fn pacific(utc: DateTime<Utc>) -> DateTime<FixedOffset> {
    let sunday = |month, nth, hour| {
        NaiveDate::from_weekday_of_month_opt(utc.year(), month, Weekday::Sun, nth)
            .and_then(|date| date.and_hms_opt(hour, 0, 0))
            .map(|time| time.and_utc())
    };
    let daylight = sunday(3, 2, 10)
        .zip(sunday(11, 1, 9))
        .is_some_and(|(start, end)| (start..end).contains(&utc));
    let hours = if daylight { 7 } else { 8 };
    utc.with_timezone(&FixedOffset::west_opt(hours * 3600).expect("within a day of UTC"))
}

/// How long ago, roughly: "just now", "5 minutes ago", "2 days ago".
fn ago(since: TimeDelta) -> String {
    if since < -TimeDelta::minutes(1) {
        return "this PC's clock is behind the build".to_string();
    }
    let count = |n: i64, unit: &str| {
        let s = if n == 1 { "" } else { "s" };
        format!("{n} {unit}{s} ago")
    };
    match since {
        s if s < TimeDelta::minutes(1) => "just now".to_string(),
        s if s < TimeDelta::hours(1) => count(s.num_minutes(), "minute"),
        s if s < TimeDelta::days(1) => count(s.num_hours(), "hour"),
        s if s < TimeDelta::days(60) => count(s.num_days(), "day"),
        s => count(s.num_days() / 30, "month"),
    }
}

fn installed(
    exe: std::io::Result<PathBuf>,
    resolve: impl FnOnce(&Path) -> std::io::Result<PathBuf>,
) -> String {
    let exe = match exe {
        Ok(exe) => exe,
        Err(e) => return format!("couldn't read the executable's path: {e}"),
    };
    match resolve(&exe) {
        Ok(real) => without_verbatim_prefix(&real.display().to_string()),
        Err(e) => format!("{} (couldn't find its real path: {e})", exe.display()),
    }
}

/// Windows names a resolved path `\\?\C:\...`, which no one would type.
fn without_verbatim_prefix(path: &str) -> String {
    if let Some(share) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
    }
}

fn checkout(name: Option<&str>, exists: &dyn Fn(&Path) -> std::io::Result<bool>) -> String {
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        return NO_CHECKOUT.to_string();
    };
    let folder = format!(r"D:\repos\{name}");
    match exists(&Path::new(&folder).join(".git")) {
        Ok(true) => folder,
        Ok(false) => NO_CHECKOUT.to_string(),
        Err(e) => format!("couldn't look at {folder}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMPED: Stamps = Stamps {
        repo: "https://forgejo.romaine.life/romaine-life/tuimail",
        version: "v0.2.6.3",
        commit: Some("0123456789abcdef0123"),
        built: Some("2026-11-02T23:14:00Z"),
    };
    const UNSTAMPED: Stamps = Stamps {
        version: "0.2.6-dev",
        commit: None,
        built: Some(""),
        ..STAMPED
    };

    fn at(time: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(time).unwrap().to_utc()
    }

    fn value(lines: &[(&'static str, String)], label: &str) -> String {
        lines.iter().find(|(l, _)| *l == label).unwrap().1.clone()
    }

    fn lines_of(stamps: Stamps, checkout: bool) -> Vec<(&'static str, String)> {
        lines_from(
            stamps,
            r"C:\Program Files\at-home\tuimail\v0.2.6.3\tuimail.exe",
            at("2026-11-04T23:14:00Z"),
            &|_| Ok(checkout),
        )
    }

    #[test]
    fn a_stamped_build_says_where_it_came_from() {
        let lines = lines_of(STAMPED, true);
        let labels: Vec<_> = lines.iter().map(|(label, _)| *label).collect();
        assert_eq!(
            labels,
            [
                "Repo",
                "Link",
                "Version",
                "Commit",
                "Built",
                "Installed",
                "Checkout"
            ]
        );
        assert_eq!(value(&lines, "Repo"), "romaine-life/tuimail");
        assert_eq!(
            value(&lines, "Link"),
            "https://forgejo.romaine.life/romaine-life/tuimail"
        );
        assert_eq!(value(&lines, "Version"), "v0.2.6.3");
        assert_eq!(value(&lines, "Commit"), "0123456789ab");
        assert_eq!(
            value(&lines, "Built"),
            "Nov 2, 3:14 PM Pacific (2 days ago)"
        );
        assert_eq!(
            value(&lines, "Installed"),
            r"C:\Program Files\at-home\tuimail\v0.2.6.3\tuimail.exe"
        );
        assert_eq!(value(&lines, "Checkout"), r"D:\repos\tuimail");
    }

    #[test]
    fn a_local_build_says_it_isnt_from_ci() {
        let lines = lines_of(UNSTAMPED, false);
        assert_eq!(value(&lines, "Version"), "0.2.6-dev");
        assert_eq!(value(&lines, "Commit"), NOT_FROM_CI);
        assert_eq!(value(&lines, "Built"), NOT_FROM_CI);
        assert_eq!(value(&lines, "Checkout"), NO_CHECKOUT);
    }

    #[test]
    fn times_read_in_pacific_and_roughly() {
        // Standard time after November's first Sunday (Nov 1, 2026).
        assert_eq!(
            built_line("2026-11-09T20:00:00Z", at("2026-11-09T20:00:30Z")),
            "Nov 9, 12:00 PM Pacific (just now)"
        );
        assert_eq!(
            built_line("2026-07-01T19:00:00Z", at("2026-07-01T21:30:00Z")),
            "Jul 1, 12:00 PM Pacific (2 hours ago)"
        );
        assert_eq!(ago(TimeDelta::minutes(1)), "1 minute ago");
        assert_eq!(ago(TimeDelta::days(90)), "3 months ago");
        assert_eq!(
            ago(-TimeDelta::hours(1)),
            "this PC's clock is behind the build"
        );
        assert!(built_line("yesterday", at("2026-11-09T20:00:00Z")).starts_with("couldn't"));
    }

    #[test]
    fn a_resolved_windows_path_reads_as_typed() {
        assert_eq!(
            without_verbatim_prefix(r"\\?\C:\x\tuimail.exe"),
            r"C:\x\tuimail.exe"
        );
        assert_eq!(
            without_verbatim_prefix(r"\\?\UNC\nas\tools\t.exe"),
            r"\\nas\tools\t.exe"
        );
        assert_eq!(
            installed(Ok(PathBuf::from("t")), |_| Err(
                std::io::ErrorKind::NotFound.into()
            )),
            "t (couldn't find its real path: entity not found)"
        );
    }
}
