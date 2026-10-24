//! Logging strategy mirror of upstream `src/services/logger.ts`.
//!
//! - `tracing` is the repo-wide log facade; **the file sink is scoped to the
//!   [`log`] facade's target** (`mem_core::logging`) — a raw `tracing` event
//!   from any other module never reaches the file.
//! - Production file logging goes through [`log`]; the composition root
//!   installs the subscriber once via [`init_process_logging`].
//! - A sink write failure after install is accepted loss: the layer uses
//!   `let _ = sink.append(..)` and never re-enters `tracing` (a logging
//!   failure must never fail a capture).
//! - File sink: env-overridden path, session banner, 5 MiB / daily rotation,
//!   30-day archive cleanup, `[<local-ISO-timestamp>] <message>[: <json>]`
//!   lines, and ENOENT re-init retry.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::TimeZone as _;
use serde_json::Value;
use tracing_subscriber::layer::Layer;

use crate::env::EnvSource;
use crate::error::LoggingError;

pub const MAX_LOG_SIZE: u64 = 5 * 1024 * 1024;
pub const MAX_LOG_DAYS: i64 = 30;

/// The tracing target the file sink accepts. Exact equality only: a prefix
/// match would re-admit sibling targets (e.g. `mem_core::logging::x`).
///
/// Private on purpose — it is an internal detail, not a public contract; the
/// public contract is [`log`].
const LOG_TARGET: &str = "mem_core::logging";

/// The nine scraped structured-log signatures, as one public spelling for the
/// writer side. The reader's classifier keeps its own literals; the mem-web
/// drift guard is the bridge between the two.
pub mod signatures;

const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f%:z";
const DATE_FORMAT: &str = "%Y-%m-%d";

/// Renders the upstream `[<local-ISO-timestamp>]` line prefix timestamp
/// (local offset, 3 ms digits).
fn format_timestamp() -> String {
    chrono::Local::now().format(TIMESTAMP_FORMAT).to_string()
}

fn format_date(date: chrono::DateTime<chrono::Local>) -> String {
    date.format(DATE_FORMAT).to_string()
}

fn local_today() -> chrono::DateTime<chrono::Local> {
    chrono::Local::now()
}

/// Renders the compact JSON payload (`data ? JSON.stringify(data)` upstream);
/// `Value::Null` is treated like the upstream falsy check (renders nothing).
fn data_json(data: Option<&Value>) -> Option<String> {
    let data = data?;
    if data.is_null() {
        return None;
    }
    serde_json::to_string(data).ok()
}

/// Internal mutable sink state.
#[derive(Debug, Default)]
struct LogSinkInner {
    initialized: bool,
    last_rotate_date: Option<String>,
}

/// Cheap-clone handle over a single log file (upstream `logger.ts`).
#[derive(Debug, Clone)]
pub struct LogSink {
    path: PathBuf,
    inner: Arc<Mutex<LogSinkInner>>,
}

impl LogSink {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            inner: Arc::new(Mutex::new(LogSinkInner::default())),
        }
    }

    /// Non-empty `OPENCODE_MEM_LOG_FILE` wins, otherwise
    /// `<home>/.opencode-mem/opencode-mem.log`.
    pub fn from_env(env: &dyn EnvSource, home: &Path) -> Self {
        match env
            .get("OPENCODE_MEM_LOG_FILE")
            .filter(|value| !value.is_empty())
        {
            Some(override_path) => Self::new(PathBuf::from(override_path)),
            None => Self::new(home.join(".opencode-mem").join("opencode-mem.log")),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn log_dir(&self) -> PathBuf {
        self.path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LogSinkInner> {
        // Never panic on a poisoned lock: the recovered state may be stale but
        // is corrected by the re-init logic.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Creates the log directory, rotates stale files, appends the session
    /// banner, and flags the sink initialized.
    pub fn init(&self) -> Result<(), LoggingError> {
        let dir = self.log_dir();
        if dir != Path::new(".") {
            fs::create_dir_all(&dir).map_err(|source| LoggingError::Io {
                path: dir.clone(),
                source,
            })?;
        } else {
            // Parent None: the log lives beside the process (upstream keeps
            // "." semantics).
            let _ = fs::create_dir_all(".");
        }
        self.rotate();
        let banner = format!("\n--- Session started: {} ---\n", format_timestamp());
        self.append(&banner).map_err(|source| LoggingError::Io {
            path: self.path.clone(),
            source,
        })?;
        let mut inner = self.lock();
        inner.initialized = true;
        inner.last_rotate_date = Some(format_date(local_today()));
        Ok(())
    }

    /// Appends one rendered line, re-initializing once when the directory
    /// vanished after init (`ENOENT`), mirroring upstream recovery.
    pub fn log(&self, message: &str, data: Option<&Value>) -> Result<(), LoggingError> {
        if !self.lock().initialized {
            self.init()?;
        }

        let today = format_date(local_today());
        {
            let mut inner = self.lock();
            if inner.last_rotate_date.as_deref() != Some(today.as_str()) {
                inner.last_rotate_date = Some(today.clone());
                drop(inner);
                self.rotate();
            }
        }

        let line = build_line(&format_timestamp(), message, data_json(data).as_deref());
        match self.append(&line) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                {
                    let mut inner = self.lock();
                    inner.initialized = false;
                }
                self.init()?;
                self.append(&line).map_err(|source| LoggingError::Io {
                    path: self.path.clone(),
                    source,
                })
            }
            Err(source) => Err(LoggingError::Io {
                path: self.path.clone(),
                source,
            }),
        }
    }

    fn append(&self, line: &str) -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())
    }

    /// Rotation: no file → nothing; rotate when size ≥ 5 MiB or the file mtime
    /// local date ≠ today; archive name carries the mtime's local date. All
    /// rotation/cleanup failures are swallowed (upstream parity).
    fn rotate(&self) {
        let Ok(meta) = fs::metadata(&self.path) else {
            return;
        };

        let need_rotate = {
            let today = format_date(local_today());
            if meta.len() >= MAX_LOG_SIZE {
                true
            } else {
                let mtime_date = meta
                    .modified()
                    .ok()
                    .map(|modified| {
                        let as_utc: chrono::DateTime<chrono::Utc> = modified.into();
                        format_date(as_utc.into())
                    })
                    .unwrap_or_default();
                mtime_date != today
            }
        };
        if !need_rotate {
            return;
        }

        let mtime_date = meta
            .modified()
            .ok()
            .map(|modified| {
                let as_utc: chrono::DateTime<chrono::Utc> = modified.into();
                format_date(as_utc.into())
            })
            .unwrap_or_else(|| format_date(local_today()));
        let archive = self
            .log_dir()
            .join(format!("opencode-mem-{mtime_date}.log"));
        if !archive.exists() {
            let _ = fs::rename(&self.path, &archive);
        } else {
            let old = self.path.with_file_name(
                self.path
                    .file_name()
                    .map(|name| format!("{}.old", name.to_string_lossy()))
                    .unwrap_or_else(|| "opencode-mem.log.old".to_owned()),
            );
            if old.exists() {
                let _ = fs::remove_file(&old);
            }
            let _ = fs::rename(&self.path, old);
        }
        self.cleanup_old_logs();
    }

    /// Deletes archives older than `MAX_LOG_DAYS` (files matching
    /// `opencode-mem-<YYYY-MM-DD>.log` whose local midnight precedes
    /// now − 30 days). Failures swallowed.
    fn cleanup_old_logs(&self) {
        let dir = self.log_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let cutoff = chrono::Local::now() - chrono::Duration::days(MAX_LOG_DAYS);
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let parsed = parse_archive_date(&name);
            let Some((y, m, d)) = parsed else { continue };
            let Some(midnight) = chrono::Local.with_ymd_and_hms(y, m, d, 0, 0, 0).earliest() else {
                continue;
            };
            if midnight < cutoff {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// `opencode-mem-YYYY-MM-DD.log` → local-midnight date parts.
fn parse_archive_date(name: &str) -> Option<(i32, u32, u32)> {
    let stem = name.strip_prefix("opencode-mem-")?.strip_suffix(".log")?;
    if stem.len() != 10
        || stem.as_bytes().get(4).copied() != Some(b'-')
        || stem.as_bytes().get(7).copied() != Some(b'-')
    {
        return None;
    }
    let y = &stem[..4];
    let m = &stem[5..7];
    let d = &stem[8..10];
    if !(y
        .bytes()
        .chain(m.bytes().chain(d.bytes()))
        .all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some((y.parse().ok()?, m.parse().ok()?, d.parse().ok()?))
}

/// Upstream line format: `[<ts>] <message>\n` or
/// `[<ts>] <message>: <compact-json>\n` (never any log level).
fn build_line(timestamp: &str, message: &str, data_json: Option<&str>) -> String {
    match data_json {
        Some(json) => format!("[{timestamp}] {message}: {json}\n"),
        None => format!("[{timestamp}] {message}\n"),
    }
}

// -------------------------------------------------------------------------
// tracing layer + global init
// -------------------------------------------------------------------------

/// A tracing layer writing rendered upstream-style lines into a [`LogSink`].
///
/// `on_event` captures the `message` and `data_json` fields (in both the
/// string and debug visitor paths) and renders through the same builder as
/// [`LogSink::log`]. Events without a `message` fall back to the event target.
/// Never panics and never calls back into `tracing` (no recursion); write
/// errors are swallowed because a layer cannot propagate.
#[derive(Debug, Clone)]
pub struct SinkLayer {
    sink: LogSink,
}

/// Field visitor capturing `message` / `data_json` from a tracing event.
#[derive(Debug, Default)]
struct EventVisitor {
    message: Option<String>,
    data_json: Option<String>,
}

impl EventVisitor {
    fn capture(&mut self, field: &tracing::field::Field, value: String) {
        match field.name() {
            "message" => self.message = Some(value),
            "data_json" => self.data_json = Some(value),
            _ => {}
        }
    }
}

impl tracing::field::Visit for EventVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.capture(field, value.to_owned());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        // Display-recorded values (`%field`) arrive here with a Debug
        // implementation that delegates to Display, so `%json` renders compact.
        self.capture(field, format!("{value:?}"));
    }
}

impl SinkLayer {
    pub fn new(sink: LogSink) -> Self {
        Self { sink }
    }
}

impl<S: tracing::Subscriber> Layer<S> for SinkLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // The file sink is a contract, not a diagnostic dump: only the facade
        // (`mem_core::logging::log`) may write lines. Exact equality keeps
        // sibling targets out.
        if event.metadata().target() != LOG_TARGET {
            return;
        }
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);
        let message = visitor
            .message
            .unwrap_or_else(|| event.metadata().target().to_owned());
        let line = build_line(&format_timestamp(), &message, visitor.data_json.as_deref());
        let _ = self.sink.append(&line);
    }
}

/// Installs the global tracing subscriber: level-filtered registry with the
/// [`SinkLayer`] writing to `sink`. `INFO` is the level later waves default
/// to; lines never carry a level (upstream format has none).
pub fn init_logging(
    sink: LogSink,
    level: tracing_subscriber::filter::LevelFilter,
) -> Result<(), LoggingError> {
    // Prime the sink first (banner + rotation + directory creation) so the
    // file contract holds even before the first event; a fresh process then
    // installs exactly once.
    sink.init()?;
    use tracing_subscriber::layer::SubscriberExt as _;
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(level)
            .with(SinkLayer::new(sink)),
    )
    .map_err(|_| LoggingError::AlreadyInstalled)
}

/// The composition-root install: pins the production level policy (`INFO`) in
/// `mem-core` and delegates to [`init_logging`].
///
/// The root package cannot name `tracing_subscriber::filter::LevelFilter`, so
/// keeping the policy here avoids leaking a dependency into the binary crate.
/// Errors are typed: an unwritable sink path surfaces as
/// [`LoggingError::Io`] and a second install as
/// [`LoggingError::AlreadyInstalled`].
pub fn init_process_logging(sink: LogSink) -> Result<(), LoggingError> {
    init_logging(sink, tracing_subscriber::filter::LevelFilter::INFO)
}

/// The canonical log call later waves use: emits an `INFO` event with
/// `message` and, when data is present, `data_json` (compact JSON via
/// Display). No subscriber installed → no-op (tracing semantics).
///
/// The event carries the [`LOG_TARGET`] target, which is what scopes the file
/// sink to this facade. Infallible by design: a logging failure must never
/// fail a capture.
pub fn log(message: &str, data: Option<&Value>) {
    match data {
        None => tracing::info!(target: LOG_TARGET, message = message),
        Some(value) => tracing::info!(target: LOG_TARGET, message = message, data_json = %value),
    }
}
