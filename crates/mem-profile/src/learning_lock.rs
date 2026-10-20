//! Cross-process profile-learning lock (W3.1 step 9) — port of upstream
//! `src/services/user-profile/learning-lock.ts`.
//!
//! Ownership is a single row in a standalone SQLite coordination database
//! (`.profile-learning-coordination.db`) inside the storage path. It is
//! deliberately separate from the memory databases: every statement here is
//! short and autonomous, no transaction is ever held across an LLM round
//! trip, and the database is opened **unencrypted** (ephemeral lock rows must
//! stay openable across key rotation) with multiprocess WAL kept on.
//!
//! - acquiring is one `INSERT … ON CONFLICT (name) DO NOTHING` (one changed
//!   row wins);
//! - a conflicting row is only taken over with a conditional
//!   `UPDATE … WHERE owner_token = <old token>` (CAS) when the recorded owner
//!   is *provably* dead: boot-id mismatch, same-kind starttime mismatch
//!   (PID reuse), or `/proc` absent + `kill(pid, 0) == ESRCH`;
//! - release is `DELETE … WHERE owner_token = <token>`, so a stale release
//!   from a previous owner (even with a reused PID) can never drop someone
//!   else's row.
//!
//! There is **no TTL and no mtime rule**: a live-but-unverifiable owner
//! (EPERM, cross-kind or unparseable identity) is never stolen, and age plays
//! no role (D-V3).
//!
//! One handle per operation, dropped at scope exit — upstream closes its
//! handle in a `finally`; turso releases on Drop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mem_store::config::StoreConfig;
use mem_store::db::{
    ConnectFactory, ConnectOptions, DbHandle, Value, query_one, turso_connect_factory,
};
use mem_store::platform::{Platform, SleepFn, system_sleep};

use crate::error::ProfileError;
use crate::ids::next_token;

/// Upstream `PROFILE_LEARNING_COORDINATION_DB`.
pub const PROFILE_LEARNING_COORDINATION_DB: &str = ".profile-learning-coordination.db";

/// The single logical lock this table ever holds (upstream `LOCK_NAME`).
pub const LOCK_NAME: &str = "profile-learning";

/// Bounded wait so a contended coordination DB cannot hang an idle handler.
pub const BUSY_TIMEOUT_MS: u64 = 5_000;

/// Upstream retry budget for lock-file contention.
pub const ACQUIRE_ATTEMPTS: usize = 8;

/// Upstream `15 + attempt * 25` ms backoff.
pub const RETRY_BASE_MS: u64 = 15;
pub const RETRY_STEP_MS: u64 = 25;

/// Upstream `CREATE TABLE IF NOT EXISTS profile_learning_lock (…)`, verbatim.
pub const LOCK_TABLE_DDL: &str = "CREATE TABLE IF NOT EXISTS profile_learning_lock (
      name TEXT PRIMARY KEY,
      owner_token TEXT NOT NULL,
      pid INTEGER NOT NULL,
      boot_id TEXT,
      starttime TEXT,
      acquired_at INTEGER NOT NULL
    )";

/// Upstream `PRAGMA busy_timeout = 5000`.
pub const BUSY_TIMEOUT_SQL: &str = "PRAGMA busy_timeout = 5000";

/// Upstream `LOCK_OPEN_ERROR_RE` fragments (case-insensitive match).
const LOCK_OPEN_ERROR_FRAGMENTS: [&str; 5] = [
    "File is locked by another process",
    "already open",
    "Locking error",
    "busy",
    "SQLITE_BUSY",
];

// ---------------------------------------------------------------------------
// Platform seams (injectable for fixtures)
// ---------------------------------------------------------------------------

/// The `kill(pid, 0)` tri-state (upstream `isPidGoneBySignal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalOutcome {
    /// `kill` returned 0 — the process exists and we may signal it.
    Alive,
    /// `EPERM` — the process exists under another uid (alive, not stealable).
    PermissionDenied,
    /// `ESRCH` — no such process: the ONLY dead outcome.
    NoSuchProcess,
    /// Anything else — unknown, never a dead signal.
    Unknown,
}

/// Signal-0 liveness probe.
pub trait SignalProbe: Send + Sync {
    fn signal0(&self, pid: u32) -> SignalOutcome;
}

/// The production probe over `libc::kill(pid, 0)`.
///
/// D-V7: `libc` is the one added direct edge (already in the dependency graph
/// via tokio); spawning `kill -0` is explicitly forbidden. EPERM must read as
/// ALIVE — a `/proc`-existence boolean cannot express that.
pub struct SystemSignalProbe;

impl SignalProbe for SystemSignalProbe {
    fn signal0(&self, pid: u32) -> SignalOutcome {
        if pid == 0 {
            return SignalOutcome::NoSuchProcess;
        }
        // SAFETY: `kill` with signal 0 performs only an existence/permission
        // check and has no side effects on the target process.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if result == 0 {
            return SignalOutcome::Alive;
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(code) if code == libc::ESRCH => SignalOutcome::NoSuchProcess,
            Some(code) if code == libc::EPERM => SignalOutcome::PermissionDenied,
            _ => SignalOutcome::Unknown,
        }
    }
}

/// One `/proc/<pid>/stat` read outcome (upstream `ProcStatResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcStat {
    /// Field 22 (starttime), decimal clock ticks.
    Ok(String),
    /// ENOENT — the pid has no `/proc` entry (hidepid=2 may hide a live one).
    Absent,
    /// Any other read/parse failure.
    Unreadable,
}

/// Process starttime identity source.
pub trait ProcStatSource: Send + Sync {
    fn read(&self, pid: u32) -> ProcStat;
}

/// The production source: field 22 of `/proc/<pid>/stat`.
pub struct SystemProcStatSource;

impl ProcStatSource for SystemProcStatSource {
    fn read(&self, pid: u32) -> ProcStat {
        let path = format!("/proc/{pid}/stat");
        let stat = match std::fs::read_to_string(&path) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ProcStat::Absent,
            Err(_) => return ProcStat::Unreadable,
        };
        // `comm` (field 2) may contain spaces and parentheses: slice after the
        // LAST ')'. Token 0 then is field 3 (state), so field 22 is token 19.
        let Some(comm_end) = stat.rfind(')') else {
            return ProcStat::Unreadable;
        };
        let rest = &stat[comm_end + 1..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        match fields.get(19) {
            Some(starttime)
                if !starttime.is_empty() && starttime.chars().all(|c| c.is_ascii_digit()) =>
            {
                ProcStat::Ok((*starttime).to_string())
            }
            _ => ProcStat::Unreadable,
        }
    }
}

/// Machine boot identity source (upstream `readBootId`).
pub trait BootIdSource: Send + Sync {
    fn read(&self) -> Option<String>;
}

/// The production source: `/proc/sys/kernel/random/boot_id`, format-validated.
///
/// Upstream's Darwin branch (`sysctl kern.boottime` → `darwin-boot-<sec>`) has
/// no Rust test platform here and is recorded N-A; a `darwin-boot-<n>` value
/// is still recognized as a *known* boot-id shape (so a foreign row is
/// compared by kind, never misread as garbage).
pub struct SystemBootIdSource;

impl BootIdSource for SystemBootIdSource {
    fn read(&self) -> Option<String> {
        let raw = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        let value = raw.trim().to_string();
        if is_known_boot_id(&value) {
            Some(value)
        } else {
            None
        }
    }
}

/// Linux boot_id / Darwin boot identity shapes (upstream `isKnownBootId`).
pub fn is_known_boot_id(value: &str) -> bool {
    is_linux_boot_id(value) || is_darwin_boot_id(value)
}

fn is_linux_boot_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    let is_hex = |byte: u8| byte.is_ascii_hexdigit();
    for (index, byte) in bytes.iter().enumerate() {
        match index {
            8 | 13 | 18 | 23 => {
                if *byte != b'-' {
                    return false;
                }
            }
            _ => {
                if !is_hex(*byte) {
                    return false;
                }
            }
        }
    }
    true
}

fn is_darwin_boot_id(value: &str) -> bool {
    match value.strip_prefix("darwin-boot-") {
        Some(seconds) => !seconds.is_empty() && seconds.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// A process identity triple (upstream `ProcessIdentity`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub boot_id: Option<String>,
    pub starttime: Option<String>,
}

/// The kind of a stored starttime identity. The two producers are platform
/// specific and their outputs are NOT comparable with each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarttimeKind {
    /// Decimal clock ticks from field 22 of `/proc/<pid>/stat`.
    Linux,
    /// `darwin:<lstart>` from `ps -o lstart=` under `LC_ALL=C`.
    Darwin,
}

/// A structurally valid stored starttime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStarttime {
    pub kind: StarttimeKind,
    pub value: String,
}

/// Upstream `parseStoredStarttime`: Linux decimal ticks or a strict Darwin
/// `lstart` behind the `darwin:` prefix. Anything else is not a comparable
/// identity and can never be used to declare an owner dead.
pub fn parse_stored_starttime(raw: &str) -> Option<ParsedStarttime> {
    if !raw.is_empty() && raw.chars().all(|c| c.is_ascii_digit()) {
        return Some(ParsedStarttime {
            kind: StarttimeKind::Linux,
            value: raw.to_string(),
        });
    }
    if let Some(lstart) = raw.strip_prefix("darwin:")
        && is_valid_darwin_lstart(lstart)
    {
        return Some(ParsedStarttime {
            kind: StarttimeKind::Darwin,
            value: lstart.to_string(),
        });
    }
    None
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Structural + calendar validation of a Darwin `lstart` (upstream
/// `isValidDarwinLstart`): `<weekday> <month> <day> <HH>:<MM>:<SS> <year>` under
/// `LC_ALL=C`, where the date must exist on the Gregorian calendar and the
/// weekday must match.
///
/// A permissive shape check alone accepts impossible dates (Oct 99, Feb 30,
/// Apr 31, Feb 29 on a non-leap year, or a wrong weekday); every one of those
/// would make a live owner's stored identity "differ" from the reader's and
/// fire a PID-reuse steal.
pub fn is_valid_darwin_lstart(lstart: &str) -> bool {
    // Collapse the space-padded day so "Oct  3" and "Oct 3" both tokenize.
    let tokens: Vec<&str> = lstart.split_whitespace().collect();
    if tokens.len() != 5 {
        return false;
    }
    let Some(weekday_index) = WEEKDAYS.iter().position(|name| *name == tokens[0]) else {
        return false;
    };
    let Some(month_index) = MONTHS.iter().position(|name| *name == tokens[1]) else {
        return false;
    };
    let Ok(day) = tokens[2].parse::<i64>() else {
        return false;
    };
    let time_parts: Vec<&str> = tokens[3].split(':').collect();
    if time_parts.len() != 3 {
        return false;
    }
    let (Ok(hour), Ok(minute), Ok(second)) = (
        time_parts[0].parse::<i64>(),
        time_parts[1].parse::<i64>(),
        time_parts[2].parse::<i64>(),
    ) else {
        return false;
    };
    let Ok(year) = tokens[4].parse::<i64>() else {
        return false;
    };
    if year < 1
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return false;
    }
    if day < 1 || day > days_in_month(year, month_index as i64) {
        return false;
    }
    let month = month_index as i64;
    weekday_of(year, month + 1, day) == weekday_index as i64
}

/// Days in a Gregorian month (century leap rule included).
fn days_in_month(year: i64, month_index: i64) -> i64 {
    const LENGTHS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if month_index == 1 {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        return if leap { 29 } else { 28 };
    }
    LENGTHS.get(month_index as usize).copied().unwrap_or(0)
}

/// Day of week for a Gregorian date (0 = Sunday), proleptic, Sakamoto's
/// algorithm — the calendar check upstream gets from `Date.UTC`.
fn weekday_of(year: i64, month: i64, day: i64) -> i64 {
    const OFFSETS: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut adjusted_year = year;
    if month < 3 {
        adjusted_year -= 1;
    }
    (adjusted_year + adjusted_year / 4 - adjusted_year / 100
        + adjusted_year / 400
        + OFFSETS[(month - 1) as usize]
        + day)
        % 7
}

// ---------------------------------------------------------------------------
// Owner-row parsing
// ---------------------------------------------------------------------------

/// Upstream `ValidOwner`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidOwner {
    pub owner_token: String,
    pub pid: u32,
    pub boot_id: Option<String>,
    pub starttime: Option<String>,
    pub parsed_starttime: Option<ParsedStarttime>,
}

/// Why a stored row was rejected (logged; never a stolen lock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerRowError {
    MissingOwnerToken,
    InvalidPid(String),
    InvalidBootId(String),
    InvalidStarttime(String),
    InvalidAcquiredAt(String),
}

impl std::fmt::Display for OwnerRowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingOwnerToken => write!(formatter, "owner_token missing/empty"),
            Self::InvalidPid(value) => write!(formatter, "invalid pid: {value}"),
            Self::InvalidBootId(value) => write!(formatter, "invalid boot_id: {value}"),
            Self::InvalidStarttime(value) => write!(formatter, "invalid starttime: {value}"),
            Self::InvalidAcquiredAt(value) => write!(formatter, "invalid acquired_at: {value}"),
        }
    }
}

/// Upstream `coercePid`: a positive integer (numeric text is tolerated).
pub fn coerce_pid(value: &Value) -> Option<u32> {
    match value {
        Value::Integer(pid) if *pid > 0 && *pid <= i64::from(u32::MAX) => Some(*pid as u32),
        Value::Text(text) => text
            .parse::<i64>()
            .ok()
            .filter(|pid| *pid > 0 && *pid <= i64::from(u32::MAX))
            .map(|pid| pid as u32),
        _ => None,
    }
}

/// Structural validation of a stored owner row. A malformed record is never
/// treated as free or reclaimable (fail-closed): the caller skips the round.
pub fn parse_owner_row(row: &mem_store::db::Row) -> Result<ValidOwner, OwnerRowError> {
    let owner_token = row
        .text("owner_token")
        .filter(|token| !token.is_empty())
        .ok_or(OwnerRowError::MissingOwnerToken)?;

    let pid_value = row.get("pid").cloned().unwrap_or(Value::Null);
    let pid = coerce_pid(&pid_value)
        .ok_or_else(|| OwnerRowError::InvalidPid(value_display(&pid_value)))?;

    let boot_value = row.get("boot_id").cloned().unwrap_or(Value::Null);
    let boot_id = match boot_value {
        Value::Null => None,
        Value::Text(value) if is_known_boot_id(&value) => Some(value),
        other => return Err(OwnerRowError::InvalidBootId(value_display(&other))),
    };

    let starttime_value = row.get("starttime").cloned().unwrap_or(Value::Null);
    let (starttime, parsed_starttime) = match starttime_value {
        Value::Null => (None, None),
        Value::Text(raw) => {
            if raw.is_empty() || raw.len() > 160 {
                return Err(OwnerRowError::InvalidStarttime(raw));
            }
            let parsed = parse_stored_starttime(&raw)
                .ok_or_else(|| OwnerRowError::InvalidStarttime(truncate(&raw, 40)))?;
            (Some(raw), Some(parsed))
        }
        other => return Err(OwnerRowError::InvalidStarttime(value_display(&other))),
    };

    let acquired_value = row.get("acquired_at").cloned().unwrap_or(Value::Null);
    match acquired_value {
        Value::Integer(_) => {}
        Value::Text(text) if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) => {}
        other => return Err(OwnerRowError::InvalidAcquiredAt(value_display(&other))),
    }

    Ok(ValidOwner {
        owner_token,
        pid,
        boot_id,
        starttime,
        parsed_starttime,
    })
}

fn value_display(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Integer(value) => value.to_string(),
        Value::Real(value) => value.to_string(),
        Value::Text(value) => value.clone(),
        Value::Blob(bytes) => format!("<blob {} bytes>", bytes.len()),
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

// ---------------------------------------------------------------------------
// Liveness
// ---------------------------------------------------------------------------

/// The three injected identity probes one acquire needs.
pub struct IdentityProbes<'a> {
    pub boot_id: &'a dyn BootIdSource,
    pub proc_stat: &'a dyn ProcStatSource,
    pub signal: &'a dyn SignalProbe,
}

/// Upstream `process.kill(pid, 0) === ESRCH` (EPERM = alive, others = unknown).
pub fn pid_gone_by_signal(probe: &dyn SignalProbe, pid: u32) -> bool {
    matches!(probe.signal0(pid), SignalOutcome::NoSuchProcess)
}

/// Upstream `ownerIsDefinitelyDead`.
///
/// True only when the recorded owner is *definitively* gone:
/// - its boot id is a known identity differing from this machine's current
///   boot id (the holder ran under a previous boot), or
/// - same-kind starttime identity is readable and differs (PID reuse), or
/// - `/proc` is absent AND the signal probe returns ESRCH (hidepid=2 makes a
///   live holder's entry invisible, so ENOENT alone is never proof), or
/// - no comparable starttime identity at all → ESRCH decides.
///
/// EPERM means "alive under another uid"; every other error is unknown. An
/// incomparable identity never produces a dead assertion and the signal probe
/// is deliberately NOT consulted on that path.
pub fn owner_is_definitely_dead(
    owner: &ValidOwner,
    identity: &ProcessIdentity,
    probes: &IdentityProbes<'_>,
) -> bool {
    if let (Some(self_boot), Some(owner_boot)) = (&identity.boot_id, &owner.boot_id)
        && owner_boot != self_boot
    {
        return true;
    }

    if let (Some(self_starttime), Some(owner_starttime)) = (&identity.starttime, &owner.starttime) {
        // Cross-kind identities are never comparable.
        let Some(owner_parsed) = owner.parsed_starttime.as_ref() else {
            return false;
        };
        let Some(self_parsed) = parse_stored_starttime(self_starttime) else {
            return false;
        };
        if self_parsed.kind != owner_parsed.kind {
            return false;
        }
        match probes.proc_stat.read(owner.pid) {
            ProcStat::Ok(live) => {
                // Never treat an unparseable live identity as PID reuse.
                match parse_stored_starttime(&live) {
                    Some(live_parsed) if live_parsed.kind == owner_parsed.kind => {
                        live != *owner_starttime
                    }
                    _ => false,
                }
            }
            ProcStat::Absent => pid_gone_by_signal(probes.signal, owner.pid),
            ProcStat::Unreadable => false,
        }
    } else {
        pid_gone_by_signal(probes.signal, owner.pid)
    }
}

// ---------------------------------------------------------------------------
// Lock
// ---------------------------------------------------------------------------

/// The post-SELECT hook signature (fixture-only).
pub type AfterSelectHook = Box<dyn Fn(Option<&str>) + Send + Sync>;

/// The pre-UPDATE hook signature (fixture-only).
pub type BeforeUpdateHook = Box<dyn Fn() + Send + Sync>;

/// The per-acquire interleaving hooks (the upstream `PLL_CAS_HOOKS` seam).
///
/// Production `try_acquire` passes `None`; only the CAS-race fixture supplies
/// hooks, so no environment variable is ever consulted.
pub struct AcquireHooks {
    /// Runs immediately after the conflicting row is selected, before it is
    /// parsed (upstream writes `hs.token.<label>` with the observed token).
    pub after_select: AfterSelectHook,
    /// Runs after the dead-owner decision and before the CAS `UPDATE`
    /// (upstream writes `hs.seen.<label>`, then parks until `hs.go.update`).
    pub before_update: BeforeUpdateHook,
}

impl AcquireHooks {
    pub fn new(
        after_select: impl Fn(Option<&str>) + Send + Sync + 'static,
        before_update: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            after_select: Box::new(after_select),
            before_update: Box::new(before_update),
        }
    }
}

/// Construction options — everything testable is injectable.
pub struct LearningLockOptions {
    pub storage_path: PathBuf,
    pub store_config: StoreConfig,
    pub factory: Arc<dyn ConnectFactory>,
    pub sleep: SleepFn,
    pub boot_id: Arc<dyn BootIdSource>,
    pub proc_stat: Arc<dyn ProcStatSource>,
    pub signal: Arc<dyn SignalProbe>,
}

impl LearningLockOptions {
    /// Production defaults for a storage root.
    pub fn new(storage_path: PathBuf, store_config: StoreConfig) -> Self {
        Self {
            storage_path,
            store_config,
            factory: Arc::new(turso_connect_factory()),
            sleep: Arc::new(system_sleep),
            boot_id: Arc::new(SystemBootIdSource),
            proc_stat: Arc::new(SystemProcStatSource),
            signal: Arc::new(SystemSignalProbe),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AcquireDecision {
    Acquired,
    Skip,
    Cas {
        old_token: String,
        previous_pid: u32,
    },
}

/// The production lock handle.
pub struct LearningLock {
    deps: Arc<LockDeps>,
}

struct LockDeps {
    storage_path: PathBuf,
    db_path: PathBuf,
    store_config: StoreConfig,
    factory: Arc<dyn ConnectFactory>,
    sleep: SleepFn,
    boot_id: Arc<dyn BootIdSource>,
    proc_stat: Arc<dyn ProcStatSource>,
    signal: Arc<dyn SignalProbe>,
}

impl LearningLock {
    pub fn new(options: LearningLockOptions) -> Self {
        let db_path = options.storage_path.join(PROFILE_LEARNING_COORDINATION_DB);
        Self {
            deps: Arc::new(LockDeps {
                storage_path: options.storage_path,
                db_path,
                store_config: options.store_config,
                factory: options.factory,
                sleep: options.sleep,
                boot_id: options.boot_id,
                proc_stat: options.proc_stat,
                signal: options.signal,
            }),
        }
    }

    /// The coordination database path.
    pub fn db_path(&self) -> &Path {
        &self.deps.db_path
    }

    /// The identity this process would record.
    pub fn current_identity(&self) -> ProcessIdentity {
        current_process_identity(self.deps.boot_id.as_ref(), self.deps.proc_stat.as_ref())
    }

    /// Serializes profile learning across every process sharing this storage.
    ///
    /// Returns `None` when another process holds the lock or the coordination
    /// state cannot be trusted (fail-closed) — both mean "skip this round; the
    /// next idle event retries". Never throws: a DB failure degrades to `None`
    /// with a log line.
    pub async fn try_acquire(&self, hooks: Option<&AcquireHooks>) -> Option<LearningGuard> {
        let identity = self.current_identity();
        let owner_token = next_token();

        let decision = match self
            .with_retry(|| async { self.acquire_decision(&identity, &owner_token, hooks).await })
            .await
        {
            Ok(decision) => decision,
            Err(error) => {
                tracing::warn!(
                    directory = %self.deps.storage_path.display(),
                    error = %error,
                    "profile-learning lock: coordination database unavailable, refusing to acquire"
                );
                return None;
            }
        };

        match decision {
            AcquireDecision::Acquired => Some(LearningGuard::new(owner_token, self.deps.clone())),
            AcquireDecision::Skip => None,
            AcquireDecision::Cas {
                old_token,
                previous_pid,
            } => {
                // Park outside the DB session so cas-race competitors are not
                // blocked by a held coordination handle.
                if let Some(hooks) = hooks {
                    (hooks.before_update)();
                }
                let claimed = match self
                    .with_retry(|| async {
                        self.cas_update(&identity, &owner_token, &old_token).await
                    })
                    .await
                {
                    Ok(claimed) => claimed,
                    Err(error) => {
                        tracing::warn!(
                            directory = %self.deps.storage_path.display(),
                            error = %error,
                            "profile-learning lock: CAS reclaim failed, skipping the round"
                        );
                        return None;
                    }
                };
                if claimed {
                    tracing::info!(
                        directory = %self.deps.storage_path.display(),
                        previous_pid,
                        "profile-learning lock: reclaimed lock from dead owner"
                    );
                    Some(LearningGuard::new(owner_token, self.deps.clone()))
                } else {
                    // Lost the CAS race to another reclaimer.
                    None
                }
            }
        }
    }

    /// Whether a coordination record currently exists. A database error is
    /// reported as "not held" but logged (upstream `isProfileLearningLockHeld`).
    pub async fn is_held(&self) -> bool {
        let result = self
            .with_retry(|| async {
                let db = self.open_coordination().await?;
                let row = query_one(
                    db.as_ref(),
                    "SELECT 1 AS ok FROM profile_learning_lock WHERE name = ? LIMIT 1",
                    vec![Value::Text(LOCK_NAME.to_string())],
                )
                .await?;
                Ok(row.is_some())
            })
            .await;
        match result {
            Ok(held) => held,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "profile-learning lock: coordination database unavailable in is_held"
                );
                false
            }
        }
    }

    // -- internals ---------------------------------------------------------

    /// `withCoordinationDbRetry`: bounded attempts, retrying only lock-file
    /// contention fragments, with the recorded `15 + 25·attempt` ms backoff.
    async fn with_retry<T, F, Fut>(&self, operation: F) -> Result<T, ProfileError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, ProfileError>>,
    {
        let mut last_error: Option<ProfileError> = None;
        for attempt in 0..ACQUIRE_ATTEMPTS {
            match operation().await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let retryable = is_coordination_lock_error(&error.to_string());
                    if attempt == ACQUIRE_ATTEMPTS - 1 || !retryable {
                        return Err(error);
                    }
                    let delay =
                        Duration::from_millis(RETRY_BASE_MS + RETRY_STEP_MS * attempt as u64);
                    (self.deps.sleep)(delay);
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or(ProfileError::LockUntrusted))
    }

    /// Upstream `withCoordinationDb`: one handle per operation, the lock table
    /// guaranteed to exist, encryption forced OFF, multiprocess WAL kept.
    async fn open_coordination(&self) -> Result<DbHandle, ProfileError> {
        if let Some(parent) = self.deps.db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut config = self.deps.store_config.clone();
        config.database_encryption_enabled = false;
        let options: ConnectOptions = ConnectOptions::for_config(&config, Platform::current());
        let db = self
            .deps
            .factory
            .connect(self.deps.db_path.clone(), options)
            .await?;
        db.execute(BUSY_TIMEOUT_SQL, Vec::new()).await?;
        db.execute(LOCK_TABLE_DDL, Vec::new()).await?;
        Ok(db)
    }

    async fn acquire_decision(
        &self,
        identity: &ProcessIdentity,
        owner_token: &str,
        hooks: Option<&AcquireHooks>,
    ) -> Result<AcquireDecision, ProfileError> {
        let db = self.open_coordination().await?;
        let inserted = db
            .execute(
                "INSERT INTO profile_learning_lock
                (name, owner_token, pid, boot_id, starttime, acquired_at)
              VALUES (?, ?, ?, ?, ?, ?)
              ON CONFLICT (name) DO NOTHING",
                vec![
                    Value::Text(LOCK_NAME.to_string()),
                    Value::Text(owner_token.to_string()),
                    Value::Integer(i64::from(identity.pid)),
                    identity.boot_id.clone().into(),
                    identity.starttime.clone().into(),
                    Value::Integer(crate::ids::now_millis()),
                ],
            )
            .await?;
        if inserted.changes == 1 {
            return Ok(AcquireDecision::Acquired);
        }

        let row = query_one(
            db.as_ref(),
            "SELECT owner_token, pid, boot_id, starttime, acquired_at
              FROM profile_learning_lock WHERE name = ?",
            vec![Value::Text(LOCK_NAME.to_string())],
        )
        .await?;
        let Some(row) = row else {
            // Released between the failed INSERT and the SELECT: contention.
            return Ok(AcquireDecision::Skip);
        };

        if let Some(hooks) = hooks {
            (hooks.after_select)(row.text("owner_token").as_deref());
        }

        let owner = match parse_owner_row(&row) {
            Ok(owner) => owner,
            Err(error) => {
                tracing::warn!(
                    directory = %self.deps.storage_path.display(),
                    reason = %error,
                    "profile-learning lock: malformed coordination record, refusing to acquire"
                );
                return Ok(AcquireDecision::Skip);
            }
        };

        let probes = self.probes();
        if !owner_is_definitely_dead(&owner, identity, &probes) {
            // Live holder, EPERM, or uncertain identity: never reclaim, never
            // guess, and no TTL is allowed to override this.
            return Ok(AcquireDecision::Skip);
        }

        Ok(AcquireDecision::Cas {
            old_token: owner.owner_token,
            previous_pid: owner.pid,
        })
    }

    async fn cas_update(
        &self,
        identity: &ProcessIdentity,
        owner_token: &str,
        old_token: &str,
    ) -> Result<bool, ProfileError> {
        let db = self.open_coordination().await?;
        let result = db
            .execute(
                "UPDATE profile_learning_lock
              SET owner_token = ?, pid = ?, boot_id = ?, starttime = ?, acquired_at = ?
              WHERE name = ? AND owner_token = ?",
                vec![
                    Value::Text(owner_token.to_string()),
                    Value::Integer(i64::from(identity.pid)),
                    identity.boot_id.clone().into(),
                    identity.starttime.clone().into(),
                    Value::Integer(crate::ids::now_millis()),
                    Value::Text(LOCK_NAME.to_string()),
                    Value::Text(old_token.to_string()),
                ],
            )
            .await?;
        Ok(result.changes == 1)
    }

    fn probes(&self) -> IdentityProbes<'_> {
        IdentityProbes {
            boot_id: self.deps.boot_id.as_ref(),
            proc_stat: self.deps.proc_stat.as_ref(),
            signal: self.deps.signal.as_ref(),
        }
    }
}

/// Upstream `currentProcessIdentity`: only a validator-clean starttime is
/// recorded (a malformed reader output degrades to "no identity").
pub fn current_process_identity(
    boot_id: &dyn BootIdSource,
    proc_stat: &dyn ProcStatSource,
) -> ProcessIdentity {
    let starttime = match proc_stat.read(std::process::id()) {
        ProcStat::Ok(starttime) if parse_stored_starttime(&starttime).is_some() => Some(starttime),
        _ => None,
    };
    ProcessIdentity {
        pid: std::process::id(),
        boot_id: boot_id.read(),
        starttime,
    }
}

/// Upstream `isCoordinationLockError`.
pub fn is_coordination_lock_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    LOCK_OPEN_ERROR_FRAGMENTS
        .iter()
        .any(|fragment| lower.contains(&fragment.to_ascii_lowercase()))
}

/// The acquired lock. `release` is token-conditional and never throws.
pub struct LearningGuard {
    owner_token: String,
    deps: Arc<LockDeps>,
    released: std::sync::atomic::AtomicBool,
}

impl LearningGuard {
    fn new(owner_token: String, deps: Arc<LockDeps>) -> Self {
        Self {
            owner_token,
            deps,
            released: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The token this guard wrote into the coordination row.
    pub fn owner_token(&self) -> &str {
        &self.owner_token
    }

    /// Whether this guard has already released.
    pub fn is_released(&self) -> bool {
        self.released.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Upstream `releaseFn`: `DELETE … WHERE owner_token = <token>`, idempotent
    /// and never throwing. On failure the row survives and is reclaimed by the
    /// dead-owner path after this process exits.
    pub async fn release(&self) {
        if self
            .released
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let lock = LearningLock {
            deps: self.deps.clone(),
        };
        let result = lock
            .with_retry(|| async {
                let db = lock.open_coordination().await?;
                db.execute(
                    "DELETE FROM profile_learning_lock WHERE name = ? AND owner_token = ?",
                    vec![
                        Value::Text(LOCK_NAME.to_string()),
                        Value::Text(self.owner_token.clone()),
                    ],
                )
                .await?;
                Ok(())
            })
            .await;
        if let Err(error) = result {
            tracing::warn!(
                error = %error,
                "profile-learning lock: release failed; row will be reclaimed after exit"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pure predicate unit tests (private-item coverage; the DB/CAS behaviour is
// asserted through the public API in `tests/profile_learning_lock.rs`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct FakeProcStat(ProcStat);
    impl ProcStatSource for FakeProcStat {
        fn read(&self, _pid: u32) -> ProcStat {
            self.0.clone()
        }
    }

    struct FakeSignal(SignalOutcome);
    impl SignalProbe for FakeSignal {
        fn signal0(&self, _pid: u32) -> SignalOutcome {
            self.0
        }
    }

    struct FakeBoot(Option<String>);
    impl BootIdSource for FakeBoot {
        fn read(&self) -> Option<String> {
            self.0.clone()
        }
    }

    fn owner(pid: u32, boot_id: Option<&str>, starttime: Option<&str>) -> ValidOwner {
        ValidOwner {
            owner_token: "planted".to_string(),
            pid,
            boot_id: boot_id.map(str::to_string),
            starttime: starttime.map(str::to_string),
            parsed_starttime: starttime.and_then(parse_stored_starttime),
        }
    }

    fn identity(pid: u32, boot_id: Option<&str>, starttime: Option<&str>) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            boot_id: boot_id.map(str::to_string),
            starttime: starttime.map(str::to_string),
        }
    }

    fn row(values: Vec<(&str, Value)>) -> mem_store::db::Row {
        let columns: Vec<String> = values.iter().map(|(name, _)| (*name).to_string()).collect();
        let values: Vec<Value> = values.into_iter().map(|(_, value)| value).collect();
        mem_store::db::Row::new(Arc::new(columns), values)
    }

    #[test]
    fn test_parse_stored_starttime_accepts_the_two_producer_formats() {
        assert_eq!(
            parse_stored_starttime("12345678"),
            Some(ParsedStarttime {
                kind: StarttimeKind::Linux,
                value: "12345678".to_string(),
            })
        );
        assert_eq!(
            parse_stored_starttime("darwin:Sat Oct  3 09:00:00 2026"),
            Some(ParsedStarttime {
                kind: StarttimeKind::Darwin,
                value: "Sat Oct  3 09:00:00 2026".to_string(),
            })
        );
        // Leap day 2028 really is a Tuesday — accepted.
        assert!(parse_stored_starttime("darwin:Tue Feb 29 09:15:02 2028").is_some());
    }

    #[test]
    fn test_parse_stored_starttime_rejects_every_malformed_form() {
        for raw in [
            "",
            "darwin:",
            "darwin:garbage",
            "darwin:not-a-date-at-all",
            "darwin:Sat Oct 3",
            "darwin:Sat Oct  3 25:61:61 2026",
            "darwin:12345",
            "darwin:Sat Oct 99 09:00:00 2026",
            "darwin:Sat Oct 00 09:00:00 2026",
            "darwin:Mon Feb 29 09:00:00 2026",
            "darwin:Sat Apr 31 09:00:00 2026",
            "darwin:Sat Oct  2 09:00:00 2026",
            "darwin:Sat Oct  3 09:00:00 0000",
            " 12345678",
            "abc",
        ] {
            assert_eq!(
                parse_stored_starttime(raw),
                None,
                "{raw:?} must not be a comparable identity"
            );
        }
    }

    #[test]
    fn test_boot_id_shape_validation() {
        assert!(is_known_boot_id("00000000-0000-0000-0000-000000000000"));
        assert!(is_known_boot_id("c1a2b3c4-d5e6-7890-abcd-ef0123456789"));
        assert!(is_known_boot_id("darwin-boot-1727936400"));
        for raw in [
            "",
            "not-a-boot-id",
            "00000000-0000-0000-0000-00000000000",
            "00000000-0000-0000-0000-0000000000000",
            "zzzzzzzz-0000-0000-0000-000000000000",
            "darwin-boot-",
            "darwin-boot-abc",
            "darwin-1234",
        ] {
            assert!(!is_known_boot_id(raw), "{raw:?} is not a known boot id");
        }
    }

    #[test]
    fn test_previous_boot_is_definitively_dead() {
        let probes = IdentityProbes {
            boot_id: &FakeBoot(None),
            proc_stat: &FakeProcStat(ProcStat::Ok("42".to_string())),
            signal: &FakeSignal(SignalOutcome::Alive),
        };
        // Same pid alive now, but the row was written under another boot.
        let owner = owner(
            4242,
            Some("00000000-0000-0000-0000-000000000000"),
            Some("42"),
        );
        let identity = identity(1, Some("c1a2b3c4-d5e6-7890-abcd-ef0123456789"), Some("42"));
        assert!(owner_is_definitely_dead(&owner, &identity, &probes));
    }

    #[test]
    fn test_cross_kind_starttime_never_kills_even_on_esrch() {
        // A Darwin-format row inspected on a Linux host: kinds are not
        // comparable, so the signal probe is deliberately not consulted.
        let probes = IdentityProbes {
            boot_id: &FakeBoot(None),
            proc_stat: &FakeProcStat(ProcStat::Ok("42".to_string())),
            signal: &FakeSignal(SignalOutcome::NoSuchProcess),
        };
        let darwin_row = owner(4194304, None, Some("darwin:Sat Oct  3 09:00:00 2026"));
        let linux_self = identity(1, None, Some("42"));
        assert!(!owner_is_definitely_dead(&darwin_row, &linux_self, &probes));

        // And the reverse direction (a Linux-format row on a Darwin host).
        let linux_row = owner(4194304, None, Some("12345678"));
        let darwin_self = identity(1, None, Some("darwin:Sat Oct  3 09:00:00 2026"));
        assert!(!owner_is_definitely_dead(&linux_row, &darwin_self, &probes));
    }

    #[test]
    fn test_same_kind_starttime_mismatch_is_pid_reuse() {
        let probes = IdentityProbes {
            boot_id: &FakeBoot(None),
            proc_stat: &FakeProcStat(ProcStat::Ok("999".to_string())),
            signal: &FakeSignal(SignalOutcome::Alive),
        };
        let owner = owner(42, None, Some("12345678"));
        let identity = identity(42, None, Some("87654321"));
        assert!(owner_is_definitely_dead(&owner, &identity, &probes));
    }

    #[test]
    fn test_same_kind_starttime_match_keeps_a_live_owner() {
        let probes = IdentityProbes {
            boot_id: &FakeBoot(None),
            proc_stat: &FakeProcStat(ProcStat::Ok("12345678".to_string())),
            signal: &FakeSignal(SignalOutcome::NoSuchProcess),
        };
        let owner = owner(42, None, Some("12345678"));
        let identity = identity(42, None, Some("12345678"));
        assert!(!owner_is_definitely_dead(&owner, &identity, &probes));
    }

    #[test]
    fn test_proc_absent_alone_never_kills_only_the_signal_probe_decides() {
        for (outcome, expected) in [
            (SignalOutcome::PermissionDenied, false),
            (SignalOutcome::Alive, false),
            (SignalOutcome::Unknown, false),
            (SignalOutcome::NoSuchProcess, true),
        ] {
            let probes = IdentityProbes {
                boot_id: &FakeBoot(None),
                proc_stat: &FakeProcStat(ProcStat::Absent),
                signal: &FakeSignal(outcome),
            };
            let owner = owner(4194304, None, Some("12345678"));
            let identity = identity(1, None, Some("1"));
            assert_eq!(
                owner_is_definitely_dead(&owner, &identity, &probes),
                expected,
                "proc absent + {outcome:?} must read as dead={expected}"
            );
        }
    }

    #[test]
    fn test_proc_unreadable_never_produces_a_dead_assertion() {
        let probes = IdentityProbes {
            boot_id: &FakeBoot(None),
            proc_stat: &FakeProcStat(ProcStat::Unreadable),
            signal: &FakeSignal(SignalOutcome::NoSuchProcess),
        };
        let owner = owner(4194304, None, Some("12345678"));
        let identity = identity(1, None, Some("1"));
        assert!(!owner_is_definitely_dead(&owner, &identity, &probes));
    }

    #[test]
    fn test_no_starttime_falls_back_to_the_signal_probe() {
        for (outcome, expected) in [
            (SignalOutcome::NoSuchProcess, true),
            (SignalOutcome::PermissionDenied, false),
            (SignalOutcome::Alive, false),
            (SignalOutcome::Unknown, false),
        ] {
            let probes = IdentityProbes {
                boot_id: &FakeBoot(None),
                proc_stat: &FakeProcStat(ProcStat::Absent),
                signal: &FakeSignal(outcome),
            };
            let owner = owner(1, None, None);
            let identity = identity(1, None, None);
            assert_eq!(
                owner_is_definitely_dead(&owner, &identity, &probes),
                expected,
                "no identity + {outcome:?}"
            );
        }
    }

    #[test]
    fn test_unparseable_live_stat_is_never_pid_reuse() {
        let probes = IdentityProbes {
            boot_id: &FakeBoot(None),
            proc_stat: &FakeProcStat(ProcStat::Ok("garbage".to_string())),
            signal: &FakeSignal(SignalOutcome::NoSuchProcess),
        };
        let owner = owner(42, None, Some("12345678"));
        let identity = identity(42, None, Some("1"));
        assert!(!owner_is_definitely_dead(&owner, &identity, &probes));
    }

    #[test]
    fn test_untrusted_local_boot_id_never_acts_as_a_dead_signal() {
        // The local boot reader returned garbage (None) — a foreign-looking
        // owner boot id must not become a dead signal.
        let probes = IdentityProbes {
            boot_id: &FakeBoot(None),
            proc_stat: &FakeProcStat(ProcStat::Ok("42".to_string())),
            signal: &FakeSignal(SignalOutcome::Alive),
        };
        let owner = owner(42, Some("00000000-0000-0000-0000-000000000000"), Some("42"));
        let identity = identity(42, None, Some("42"));
        assert!(!owner_is_definitely_dead(&owner, &identity, &probes));
    }

    #[test]
    fn test_parse_owner_row_rejects_malformed_records() {
        let valid = row(vec![
            ("owner_token", Value::Text("token".to_string())),
            ("pid", Value::Integer(42)),
            ("boot_id", Value::Null),
            ("starttime", Value::Null),
            ("acquired_at", Value::Integer(1)),
        ]);
        assert!(parse_owner_row(&valid).is_ok());

        let cases: Vec<(mem_store::db::Row, OwnerRowError)> = vec![
            (
                row(vec![
                    ("owner_token", Value::Text(String::new())),
                    ("pid", Value::Integer(42)),
                    ("boot_id", Value::Null),
                    ("starttime", Value::Null),
                    ("acquired_at", Value::Integer(1)),
                ]),
                OwnerRowError::MissingOwnerToken,
            ),
            (
                row(vec![
                    ("owner_token", Value::Text("token".to_string())),
                    ("pid", Value::Integer(0)),
                    ("boot_id", Value::Null),
                    ("starttime", Value::Null),
                    ("acquired_at", Value::Integer(1)),
                ]),
                OwnerRowError::InvalidPid("0".to_string()),
            ),
            (
                row(vec![
                    ("owner_token", Value::Text("token".to_string())),
                    ("pid", Value::Integer(42)),
                    ("boot_id", Value::Text("garbage".to_string())),
                    ("starttime", Value::Null),
                    ("acquired_at", Value::Integer(1)),
                ]),
                OwnerRowError::InvalidBootId("garbage".to_string()),
            ),
            (
                row(vec![
                    ("owner_token", Value::Text("token".to_string())),
                    ("pid", Value::Integer(42)),
                    ("boot_id", Value::Null),
                    ("starttime", Value::Text("darwin:garbage".to_string())),
                    ("acquired_at", Value::Integer(1)),
                ]),
                OwnerRowError::InvalidStarttime("darwin:garbage".to_string()),
            ),
            (
                row(vec![
                    ("owner_token", Value::Text("token".to_string())),
                    ("pid", Value::Integer(42)),
                    ("boot_id", Value::Null),
                    ("starttime", Value::Null),
                    ("acquired_at", Value::Text("later".to_string())),
                ]),
                OwnerRowError::InvalidAcquiredAt("later".to_string()),
            ),
        ];
        for (row, expected) in cases {
            assert_eq!(parse_owner_row(&row).unwrap_err(), expected);
        }
    }

    #[test]
    fn test_coordination_lock_error_fragments() {
        assert!(is_coordination_lock_error("SQLITE_BUSY: database is busy"));
        assert!(is_coordination_lock_error(
            "File is locked by another process"
        ));
        assert!(is_coordination_lock_error("Locking error"));
        assert!(!is_coordination_lock_error("no such table: nope"));
        assert!(!is_coordination_lock_error("disk I/O error"));
    }

    #[test]
    fn test_lock_table_ddl_is_the_upstream_shape() {
        for column in [
            "name TEXT PRIMARY KEY",
            "owner_token TEXT NOT NULL",
            "pid INTEGER NOT NULL",
            "boot_id TEXT",
            "starttime TEXT",
            "acquired_at INTEGER NOT NULL",
        ] {
            assert!(LOCK_TABLE_DDL.contains(column), "missing {column}");
        }
        assert_eq!(BUSY_TIMEOUT_SQL, "PRAGMA busy_timeout = 5000");
    }
}
