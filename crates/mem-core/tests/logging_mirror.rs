//! Mirror tests for the upstream `logger.ts` behaviors: env-overridden path,
//! session banner, line format, size/date rotation, collision fallback,
//! 30-day cleanup, and ENOENT re-init.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mem_core::logging::LogSink;
mod common;

type Env = std::collections::BTreeMap<String, String>;

// --- std-only date helpers (no chrono in dev-dependencies) ------------------

/// Howard Hinnant's `days_from_civil` (dates in the modern era are all
/// positive, so negative-division corner cases do not apply).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn format_date(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn utc_days_now() -> i64 {
    (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 86_400) as i64
}

fn epoch_noon(days: i64) -> SystemTime {
    // Local noon of that date: identical local date for every timezone.
    UNIX_EPOCH + Duration::from_secs((days * 86_400 + 43_200) as u64)
}

/// `opencode-mem-YYYY-MM-DD.log` → `(y, m, d)`.
fn archive_date(name: &str) -> Option<(i64, i64, i64)> {
    let stem = name.strip_prefix("opencode-mem-")?.strip_suffix(".log")?;
    if stem.len() != 10 {
        return None;
    }
    let bytes = stem.as_bytes();
    let digits = |slice: &str| slice.bytes().all(|b| b.is_ascii_digit());
    if !(bytes[4] == b'-'
        && bytes[7] == b'-'
        && digits(&stem[..4])
        && digits(&stem[5..7])
        && digits(&stem[8..10]))
    {
        return None;
    }
    Some((
        stem[..4].parse().ok()?,
        stem[5..7].parse().ok()?,
        stem[8..10].parse().ok()?,
    ))
}

/// `2026-10-18T11:05:22.123+02:00` shape (local offset, 3 ms digits — the
/// upstream `[<local-ISO-timestamp>]` line prefix).
fn is_iso_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 29 {
        return false;
    }
    let all = |range: std::ops::Range<usize>, out: &mut String| -> bool {
        range.clone().all(|i| bytes[i].is_ascii_digit()) && {
            out.push_str(&value[range]);
            true
        }
    };
    let (mut y, mut m, mut d, mut hh, mut mm, mut ss, mut ms, mut oh, mut om): (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    ) = Default::default();
    all(0..4, &mut y)
        && bytes[4] == b'-'
        && all(5..7, &mut m)
        && bytes[7] == b'-'
        && all(8..10, &mut d)
        && bytes[10] == b'T'
        && all(11..13, &mut hh)
        && bytes[13] == b':'
        && all(14..16, &mut mm)
        && bytes[16] == b':'
        && all(17..19, &mut ss)
        && bytes[19] == b'.'
        && all(20..23, &mut ms)
        && (bytes[23] == b'+' || bytes[23] == b'-')
        && all(24..26, &mut oh)
        && bytes[26] == b':'
        && all(27..29, &mut om)
        && y.parse::<i64>().is_ok()
        && (2020..2100).contains(&y.parse::<i64>().unwrap())
        && (1..=12).contains(&m.parse::<i64>().unwrap())
        && (1..=31).contains(&d.parse::<i64>().unwrap())
        && oh.parse::<i64>().is_ok()
        && om.parse::<i64>().is_ok()
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut names: Vec<PathBuf> = fs::read_dir(dir)
        .expect("read log dir")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    names.sort();
    names
}

fn seed_log_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("seed parent dir");
    }
    fs::write(path, contents).expect("seed log file");
}

// --- tests -------------------------------------------------------------------

#[test]
fn test_env_override_and_default_path() {
    // Empty env → default `$HOME/.opencode-mem/opencode-mem.log`.
    let home = common::unique_temp_dir("log-default-home");
    let sink = LogSink::from_env(&Env::new(), &home);
    assert_eq!(
        sink.path(),
        home.join(".opencode-mem")
            .join("opencode-mem.log")
            .as_path()
    );

    // Non-empty override wins.
    let home = common::unique_temp_dir("log-override-home");
    let env = Env::from([(
        "OPENCODE_MEM_LOG_FILE".to_owned(),
        "/custom/logs/example.log".to_owned(),
    )]);
    let sink = LogSink::from_env(&env, &home);
    assert_eq!(sink.path(), Path::new("/custom/logs/example.log"));

    // Empty-string override falls back to the default (upstream truthiness).
    let home = common::unique_temp_dir("log-empty-home");
    let env = Env::from([("OPENCODE_MEM_LOG_FILE".to_owned(), String::new())]);
    let sink = LogSink::from_env(&env, &home);
    assert_eq!(
        sink.path(),
        home.join(".opencode-mem")
            .join("opencode-mem.log")
            .as_path()
    );
}

#[test]
fn test_init_writes_session_banner() {
    let dir = common::unique_temp_dir("log-banner");
    let path = dir.join("opencode-mem.log");
    let sink = LogSink::new(path.clone());
    fs::write(&path, "previous session content\n").expect("seed");
    sink.init().expect("init");
    let content = fs::read_to_string(&path).expect("read");
    assert!(content.starts_with("previous session content\n"));
    // The banner is appended after the seeded content; the leading "\n" of the
    // banner leaves a blank separator line, so take the LAST line.
    let banner_line = content.lines().last().expect("banner line");
    assert!(
        banner_line.starts_with("--- Session started: "),
        "{banner_line}"
    );
    let timestamp = banner_line
        .strip_prefix("--- Session started: ")
        .expect("prefix")
        .strip_suffix(" ---")
        .expect("suffix");
    assert!(is_iso_timestamp(timestamp), "banner ts: {timestamp}");
    assert!(content.ends_with("---\n"), "banner ends with newline");
}

#[test]
fn test_log_line_exact_shape_with_and_without_data() {
    let dir = common::unique_temp_dir("log-lines");
    let sink = LogSink::new(dir.join("opencode-mem.log"));
    sink.init().expect("init");

    sink.log("captured memory", None).expect("log plain");
    sink.log(
        "flushed",
        Some(&serde_json::json!({ "a": 1, "b": [",}", ",]"] })),
    )
    .expect("log with data");

    let content = fs::read_to_string(dir.join("opencode-mem.log")).expect("read");
    // Blank line between banner and events is cosmetic only (banner prefix).
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(lines.len() >= 3, "{content}");
    assert!(lines[0].starts_with("--- Session started: "));
    assert!(is_iso_timestamp(
        lines[0]
            .strip_prefix("--- Session started: ")
            .unwrap()
            .strip_suffix(" ---")
            .unwrap()
    ));

    // No data → `[<ts>] <message>\n`.
    let (ts, message) = lines[1]
        .strip_prefix('[')
        .expect("open bracket")
        .split_once("] ")
        .expect("close bracket");
    assert!(is_iso_timestamp(ts), "ts: {ts}");
    assert_eq!(message, "captured memory");

    // With data → `[<ts>] <message>: <compact-json>\n`.
    let line2 = lines[2];
    let (ts2, rest) = line2.strip_prefix('[').unwrap().split_once("] ").unwrap();
    assert!(is_iso_timestamp(ts2), "ts: {ts2}");
    assert_eq!(rest, r#"flushed: {"a":1,"b":[",}",",]"]}"#);

    // Lines never carry log levels (no "INFO"/"DEBUG" prefixes).
    assert!(!content.contains("INFO"), "{content}");
}

#[test]
fn test_log_never_panics_without_init() {
    // `log` initializes on first use — no explicit init needed.
    let dir = common::unique_temp_dir("log-lazy");
    let sink = LogSink::new(dir.join("opencode-mem.log"));
    sink.log("lazy init line", None).expect("log");
    let content = fs::read_to_string(dir.join("opencode-mem.log")).expect("read");
    assert!(content.contains("lazy init line"), "{content}");
    assert!(content.contains("--- Session started: "));
}

#[test]
fn test_size_rotation_archives_and_starts_fresh() {
    let dir = common::unique_temp_dir("log-size-rotate");
    let path = dir.join("opencode-mem.log");
    let oversized = vec![b'x'; 5 * 1024 * 1024];
    fs::write(&path, oversized).expect("seed oversized log");

    let sink = LogSink::new(path.clone());
    sink.init().expect("init");

    let entries = files_in(&dir);
    assert!(
        entries
            .iter()
            .any(|entry| entry.file_name().unwrap() == "opencode-mem.log")
    );
    let archives: Vec<&PathBuf> = entries
        .iter()
        .filter(|entry| archive_date(&entry.file_name().unwrap().to_string_lossy()).is_some())
        .collect();
    assert_eq!(
        archives.len(),
        1,
        "one archive after size rotation: {entries:?}"
    );
    let archived = fs::metadata(archives[0]).expect("archive meta").len();
    assert_eq!(
        archived,
        5 * 1024 * 1024,
        "archive holds the original 5 MiB"
    );

    // Fresh live log with the session banner.
    let content = fs::read_to_string(&path).expect("fresh log");
    assert!(
        content.starts_with("\n--- Session started: "),
        "{content:?}"
    );
}

#[test]
fn test_date_change_rotation_archives_under_mtime_date() {
    let dir = common::unique_temp_dir("log-date-rotate");
    let path = dir.join("opencode-mem.log");
    seed_log_file(&path, "two-days-old session\n");
    let past = epoch_noon(utc_days_now() - 2);
    File::open(&path)
        .unwrap()
        .set_modified(past)
        .expect("set mtime");

    let sink = LogSink::new(path.clone());
    sink.init().expect("init");

    let entries = files_in(&dir);
    let archives: Vec<&PathBuf> = entries
        .iter()
        .filter(|entry| archive_date(&entry.file_name().unwrap().to_string_lossy()).is_some())
        .collect();
    assert_eq!(archives.len(), 1, "mtime-date archive name: {entries:?}");
    assert!(
        fs::read_to_string(archives[0])
            .is_ok_and(|content| content.contains("two-days-old session")),
        "archive holds the old content"
    );
    let content = fs::read_to_string(&path).expect("fresh log");
    assert!(
        content.starts_with("\n--- Session started: "),
        "{content:?}"
    );
}

#[test]
fn test_collision_falls_back_to_old_file() {
    let dir = common::unique_temp_dir("log-collision");
    let path = dir.join("opencode-mem.log");
    seed_log_file(&path, "first session\n");
    let past = epoch_noon(utc_days_now() - 2);
    File::open(&path)
        .unwrap()
        .set_modified(past)
        .expect("set mtime");

    // First rotation produces the archive whose name the second rotation would
    // reuse — force the collision deterministically.
    let sink = LogSink::new(path.clone());
    sink.init().expect("first init rotates");
    let entries = files_in(&dir);
    let archive_path = entries
        .iter()
        .find(|entry| archive_date(&entry.file_name().unwrap().to_string_lossy()).is_some())
        .expect("first archive")
        .clone();
    let (y, m, d) = archive_date(&archive_path.file_name().unwrap().to_string_lossy()).unwrap();

    // Second seed whose mtime date collides with the existing archive name, plus a stale .old.
    seed_log_file(&path, "second session\n");
    File::open(&path)
        .unwrap()
        .set_modified(epoch_noon(days_from_civil(y, m, d)))
        .expect("set mtime");
    seed_log_file(&dir.join("opencode-mem.log.old"), "stale old content\n");

    sink.init().expect("second init collides");

    // Space-making only touches `.old`; the first archive is preserved.
    assert_eq!(
        fs::read_to_string(&archive_path).expect("archive intact"),
        "first session\n"
    );
    let old_path = dir.join("opencode-mem.log.old");
    assert_eq!(
        fs::read_to_string(&old_path).expect("old file"),
        "second session\n"
    );
    // The live file was recreated with the banner.
    let content = fs::read_to_string(&path).expect("fresh log");
    assert!(
        content.starts_with("\n--- Session started: "),
        "{content:?}"
    );
}

#[test]
fn test_cleanup_removes_very_old_archives_keeps_recent() {
    let dir = common::unique_temp_dir("log-cleanup");
    let path = dir.join("opencode-mem.log");
    fs::write(&path, "will be rotated away\n").expect("seed");
    File::open(&path)
        .unwrap()
        .set_modified(epoch_noon(utc_days_now() - 2))
        .expect("set mtime");

    // Seeds on both sides of the 30-day cutoff.
    common::write_file(&dir, "opencode-mem-2020-01-01.log", "ancient\n");
    common::write_file(&dir, "opencode-mem-other.log", "not an archive name\n");
    let recent = format!("opencode-mem-{}.log", format_date(utc_days_now()));
    common::write_file(&dir, &recent, "recent\n");

    let sink = LogSink::new(path);
    sink.init().expect("init");

    assert!(
        !dir.join("opencode-mem-2020-01-01.log").exists(),
        "ancient archive must be deleted"
    );
    assert!(
        dir.join("opencode-mem-other.log").exists(),
        "non-matching files are untouched"
    );
    assert!(dir.join(&recent).exists(), "recent archive is kept");
}

#[test]
fn test_enoent_recovery_recreates_directory_and_banner() {
    let dir = common::unique_temp_dir("log-enoent");
    let sink = LogSink::new(dir.join("opencode-mem.log"));
    sink.log("before removal", None).expect("log");

    fs::remove_dir_all(&dir).expect("simulate external dir removal");

    sink.log("after removal", None)
        .expect("re-init on NotFound");
    let content = fs::read_to_string(dir.join("opencode-mem.log")).expect("read recreated");
    assert!(content.contains("--- Session started: "), "{content}");
    assert!(content.contains("after removal"), "{content}");
    assert!(
        content.starts_with("\n--- Session started: "),
        "{content:?}"
    );
}
