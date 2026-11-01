//! Host endpoint discovery — resolve the LIVE OpenCode host.
//!
//! The engine must talk to the host that is actually running. `opencode serve`
//! records itself in `~/.local/state/opencode/service.json` (`0600`) as
//! `{id, url, pid, password, version}`; the port's API lives at
//! `{origin}/api` and requires HTTP basic auth (`opencode` + that password).
//! Nothing about that is guessable, so this module resolves it lazily instead
//! of dialling a compiled-in default.
//!
//! Two **disjoint** branches (never fall-through):
//!
//! - **A** — a non-blank `opencodeBaseUrl` override is the authority. It is
//!   validated (loopback, explicit port, supported path) and the service file is
//!   consulted for **nothing but credential binding**; a different-origin
//!   override gets no credentials at all, and no liveness probe runs.
//! - **B** — the service file is the authority: state dir → file → pid liveness
//!   → pid recycling → URL shape → credentials → model ref.
//!
//! Every failure is a member of the closed [`HostWiringError`] set. The
//! password is wrapped in [`Zeroizing`] immediately after the deserialize, in
//! the ONE shared read+parse helper both branches use, so no later rule can
//! return while holding a plaintext `String`.

use std::fmt;
use std::path::{Path, PathBuf};

use mem_core::env::{EnvSource, home_dir};
use mem_store::platform::ProcessProbe;
use url::Url;
use zeroize::Zeroizing;

use mem_ai::provider::HostAuthorization;

/// The state-root environment variable (`XDG_STATE_HOME`).
pub const STATE_DIR_ENV: &str = "XDG_STATE_HOME";

/// The host's service record inside `<state>/opencode/`.
pub const SERVICE_FILE: &str = "service.json";

/// The API path prefix the provider appends `/experimental/generate/text` and
/// `/config` to.
pub const API_PREFIX: &str = "api";

/// The basic-auth user name the host accepts.
pub const BASIC_AUTH_USER: &str = "opencode";

/// Everything the provider needs to reach the live host.
pub struct HostWiring {
    /// `"{origin}/api"`, no trailing slash.
    pub base_url: String,
    /// Provider id the session prompt names.
    pub provider_id: String,
    /// Model id the session prompt names.
    pub model_id: String,
    /// `service.json`'s version, bounded — `None` when unknown or unreadable
    /// (and always `None` on branch A, where the file is credential-only).
    pub host_version: Option<String>,
    /// Basic-auth credential, when a resolvable service file bound one.
    pub authorization: Option<HostAuthorization>,
}

// A manual `Debug` on purpose: the credential field prints a placeholder, never
// the value and never its base64 header.
impl fmt::Debug for HostWiring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostWiring")
            .field("base_url", &self.base_url)
            .field("provider_id", &self.provider_id)
            .field("model_id", &self.model_id)
            .field("host_version", &self.host_version)
            .field(
                "authorization",
                &self.authorization.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// The closed host-resolution reason set (ten variants, ten distinct strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostWiringError {
    /// Branch A only: the operator's override is a usable loopback http URL
    /// but its path is not a supported base.
    OverrideInvalid,
    /// Branch B only: neither `XDG_STATE_HOME` nor `HOME`/`USERPROFILE`.
    StateDirUnknown,
    /// Branch B only: a valid state dir with no `service.json`.
    ServiceFileMissing,
    /// Both branches: an I/O failure that is not "missing".
    ServiceFileUnreadable,
    /// Both branches: unparsable JSON, a missing `pid`/`password`, or `pid: 0`.
    ServiceFileMalformed,
    /// Branch B only: the recorded pid is not alive.
    PidDead,
    /// Branch B only: the recorded pid started AFTER the engine, so it was
    /// recycled (the host always starts before the engine it spawns).
    PidRecycled,
    /// Both branches: the URL is not an http(s) URL with an explicit port and a
    /// supported path (`""`, `/`, `api`, `/api`).
    UrlUnsupported,
    /// Both branches: the host is not `127.0.0.1`, `::1` or `localhost`, or the
    /// URL smuggles userinfo.
    UrlNotLoopback,
    /// Both branches: no explicit model, or the literal `"inherit"`.
    ModelUnresolved,
}

impl HostWiringError {
    /// The stable, content-free reason string carried into the log and the
    /// dead-letter payload.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::OverrideInvalid => "host_override_invalid",
            Self::StateDirUnknown => "state_dir_unknown",
            Self::ServiceFileMissing => "service_file_missing",
            Self::ServiceFileUnreadable => "service_file_unreadable",
            Self::ServiceFileMalformed => "service_file_malformed",
            Self::PidDead => "host_pid_dead",
            Self::PidRecycled => "host_pid_recycled",
            Self::UrlUnsupported => "host_url_unsupported",
            Self::UrlNotLoopback => "host_url_not_loopback",
            Self::ModelUnresolved => "host_model_unresolved",
        }
    }

    /// Every variant, for the drift guard that pins one input per reason.
    pub fn all() -> [Self; 10] {
        [
            Self::OverrideInvalid,
            Self::StateDirUnknown,
            Self::ServiceFileMissing,
            Self::ServiceFileUnreadable,
            Self::ServiceFileMalformed,
            Self::PidDead,
            Self::PidRecycled,
            Self::UrlUnsupported,
            Self::UrlNotLoopback,
            Self::ModelUnresolved,
        ]
    }
}

impl fmt::Display for HostWiringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for HostWiringError {}

/// A resolution failure plus the host version when the service file had already
/// parsed — so the "provider unavailable" line can name the API generation that
/// failed the resolution.
#[derive(Debug)]
pub struct HostResolutionError {
    /// The named reason.
    pub error: HostWiringError,
    /// `service.json`'s bounded version, when it was readable.
    pub host_version: Option<String>,
}

impl HostResolutionError {
    fn new(error: HostWiringError, host_version: Option<&String>) -> Self {
        Self {
            error,
            host_version: host_version.cloned(),
        }
    }
}

/// Process start-time oracle (pid recycling detection).
pub trait ProcessClock: Send + Sync {
    /// The kernel's start time for `pid` in clock ticks since boot, or `None`
    /// when it cannot be observed (process gone, or a non-procfs host).
    fn start_time(&self, pid: u32) -> Option<u64>;
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProcessClock;

impl ProcessClock for SystemProcessClock {
    fn start_time(&self, pid: u32) -> Option<u64> {
        mem_store::platform::process_start_time(pid)
    }
}

/// What the caller already knows about the host, from the resolved config.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostWiringRequest<'a> {
    /// `opencodeBaseUrl`; blank is unset.
    pub override_url: Option<&'a str>,
    /// `opencodeProvider`.
    pub configured_provider: Option<&'a str>,
    /// `opencodeModel`.
    pub configured_model: Option<&'a str>,
}

/// Resolve the host wiring, or its named reason.
pub fn resolve_host_wiring(
    env: &(dyn EnvSource + Send + Sync),
    probe: &dyn ProcessProbe,
    clock: &dyn ProcessClock,
    request: &HostWiringRequest<'_>,
) -> Result<HostWiring, HostWiringError> {
    resolve_host_wiring_detailed(env, probe, clock, request).map_err(|failure| failure.error)
}

/// [`resolve_host_wiring`] plus the host version when it was readable.
///
/// Split out (rather than folded into [`HostWiringError`]) because the version
/// is diagnostic data, not part of the closed reason set: the enum stays `Copy`
/// and every variant keeps exactly one reason string.
pub fn resolve_host_wiring_detailed(
    env: &(dyn EnvSource + Send + Sync),
    probe: &dyn ProcessProbe,
    clock: &dyn ProcessClock,
    request: &HostWiringRequest<'_>,
) -> Result<HostWiring, HostResolutionError> {
    let state_dir = state_dir(env);
    match request
        .override_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        Some(override_url) => resolve_override(override_url, state_dir.as_deref(), request),
        None => resolve_service_file(state_dir.as_deref(), probe, clock, request),
    }
}

/// Branch A — the override is the authority, the file is credential-only.
fn resolve_override(
    override_url: &str,
    state_dir: Option<&Path>,
    request: &HostWiringRequest<'_>,
) -> Result<HostWiring, HostResolutionError> {
    let url = Url::parse(override_url)
        .map_err(|_| HostResolutionError::new(HostWiringError::UrlUnsupported, None))?;
    validate_shape(&url).map_err(|error| HostResolutionError::new(error, None))?;
    if !supported_path(url.path()) {
        // The operator named an unusable base — a distinct reason from the
        // host writing one, because only the operator can fix this.
        return Err(HostResolutionError::new(
            HostWiringError::OverrideInvalid,
            None,
        ));
    }
    let base_url = api_base(&url);

    let authorization = match read_service_file(state_dir) {
        Ok(file) => {
            if file.url.origin() == url.origin() {
                Some(HostAuthorization::basic_from_owned(
                    BASIC_AUTH_USER,
                    file.password,
                ))
            } else {
                // Same secret, different host: never send it.
                None
            }
        }
        // No resolvable state dir, or no file: the override still resolves.
        Err(HostWiringError::ServiceFileMissing) => None,
        Err(error) => return Err(HostResolutionError::new(error, None)),
    };

    let (provider_id, model_id) =
        model_ref(request).map_err(|error| HostResolutionError::new(error, None))?;
    Ok(HostWiring {
        base_url,
        provider_id,
        model_id,
        // The file is credential-only on this branch: its `version` describes
        // whatever host wrote it, not necessarily the override's.
        host_version: None,
        authorization,
    })
}

/// Branch B — the service file is the authority.
fn resolve_service_file(
    state_dir: Option<&Path>,
    probe: &dyn ProcessProbe,
    clock: &dyn ProcessClock,
    request: &HostWiringRequest<'_>,
) -> Result<HostWiring, HostResolutionError> {
    let Some(state_dir) = state_dir else {
        return Err(HostResolutionError::new(
            HostWiringError::StateDirUnknown,
            None,
        ));
    };
    let file = read_service_file(Some(state_dir))
        .map_err(|error| HostResolutionError::new(error, None))?;
    let version = file.version.clone();

    if !probe.is_alive(file.pid) {
        return Err(HostResolutionError::new(
            HostWiringError::PidDead,
            version.as_ref(),
        ));
    }
    // The host starts BEFORE the engine it spawns, so a recorded start time
    // later than the engine's is a recycled pid — a different process wearing
    // the host's number.
    if let (Some(recorded), Some(engine)) = (
        clock.start_time(file.pid),
        clock.start_time(std::process::id()),
    ) && recorded > engine
    {
        return Err(HostResolutionError::new(
            HostWiringError::PidRecycled,
            version.as_ref(),
        ));
    }

    validate_shape(&file.url).map_err(|error| HostResolutionError::new(error, version.as_ref()))?;
    if !supported_path(file.url.path()) {
        // The host wrote a sub-path: never silently stripped, never doubled.
        return Err(HostResolutionError::new(
            HostWiringError::UrlUnsupported,
            version.as_ref(),
        ));
    }
    let base_url = api_base(&file.url);
    let (provider_id, model_id) =
        model_ref(request).map_err(|error| HostResolutionError::new(error, version.as_ref()))?;
    let authorization = Some(HostAuthorization::basic_from_owned(
        BASIC_AUTH_USER,
        file.password,
    ));
    Ok(HostWiring {
        base_url,
        provider_id,
        model_id,
        host_version: file.version,
        authorization,
    })
}

/// The parsed service file. The password is `Zeroizing` from construction on.
struct ServiceFile {
    url: Url,
    pid: u32,
    password: Zeroizing<String>,
    version: Option<String>,
}

/// `service.json` as the wire sees it. `Deserialize`-only ON PURPOSE: no
/// `Debug`, never through `serde_json::Value`, unknown keys ignored.
#[derive(serde::Deserialize)]
struct ServiceFileWire {
    url: String,
    pid: u64,
    password: String,
    #[serde(default)]
    version: Option<String>,
}

/// The ONE read+parse path both branches use, and the only place the password
/// is touched.
///
/// The order is load-bearing: read the text into `Zeroizing` → deserialize →
/// **wrap the password immediately** → only then the `pid` and URL
/// validations. Every rule that can return `Err` after the parse already holds
/// a zeroizing secret.
fn read_service_file(state_dir: Option<&Path>) -> Result<ServiceFile, HostWiringError> {
    let Some(state_dir) = state_dir else {
        return Err(HostWiringError::ServiceFileMissing);
    };
    let path = state_dir.join("opencode").join(SERVICE_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => Zeroizing::new(text),
        // A directory (or an unreadable file) is NOT "missing": the state dir
        // is there, so the cause is different and must be named differently.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(HostWiringError::ServiceFileMissing);
        }
        Err(_) => return Err(HostWiringError::ServiceFileUnreadable),
    };
    let wire: ServiceFileWire =
        serde_json::from_str(&text).map_err(|_| HostWiringError::ServiceFileMalformed)?;
    let password = Zeroizing::new(wire.password);
    if wire.pid == 0 || wire.pid > u64::from(u32::MAX) {
        return Err(HostWiringError::ServiceFileMalformed);
    }
    let url = Url::parse(&wire.url).map_err(|_| HostWiringError::UrlUnsupported)?;
    Ok(ServiceFile {
        url,
        pid: wire.pid as u32,
        password,
        version: bounded_version(wire.version),
    })
}

/// The URL rules both branches share.
fn validate_shape(url: &Url) -> Result<(), HostWiringError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(HostWiringError::UrlUnsupported);
    }
    // Userinfo is an identity smuggle: loopback is not identity, and the whole
    // `http://127.0.0.1:port@evil.example` class lands here.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HostWiringError::UrlNotLoopback);
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(HostWiringError::UrlUnsupported);
    }
    if url.port().is_none() {
        return Err(HostWiringError::UrlUnsupported);
    }
    match url.host_str() {
        Some("127.0.0.1" | "::1" | "localhost") => Ok(()),
        _ => Err(HostWiringError::UrlNotLoopback),
    }
}

/// The supported base paths: the origin itself, or the `/api` prefix.
fn supported_path(path: &str) -> bool {
    matches!(path, "" | "/" | API_PREFIX | "/api")
}

/// `"{origin}/api"` — one spelling, never doubled.
fn api_base(url: &Url) -> String {
    format!("{}/{}", url.origin().ascii_serialization(), API_PREFIX)
}

/// The model ref the session prompt will name, or the fail-closed reason.
///
/// `"inherit"` is deliberately NOT forwarded: the host would receive a literal
/// model id and reject the prompt opaquely.
fn model_ref(request: &HostWiringRequest<'_>) -> Result<(String, String), HostWiringError> {
    let model = request
        .configured_model
        .map(str::trim)
        .filter(|model| !model.is_empty() && *model != "inherit")
        .ok_or(HostWiringError::ModelUnresolved)?;
    let provider = request
        .configured_provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .unwrap_or("opencode");
    Ok((provider.to_owned(), model.to_owned()))
}

/// The state root: `XDG_STATE_HOME` (non-empty) else `$HOME/.local/state`.
fn state_dir(env: &(dyn EnvSource + Send + Sync)) -> Option<PathBuf> {
    if let Some(dir) = env.get(STATE_DIR_ENV).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    home_dir(env).map(|home| home.join(".local").join("state"))
}

/// `hostVersion` is logged, so it is bounded: `[0-9A-Za-z.-]`, ≤ 32 chars.
fn bounded_version(version: Option<String>) -> Option<String> {
    version.filter(|value| {
        !value.is_empty()
            && value.len() <= 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_version_rejects_free_text() {
        assert_eq!(
            bounded_version(Some("2.2.5".to_owned())),
            Some("2.2.5".to_owned())
        );
        assert_eq!(bounded_version(Some("2.2.5 nightly".to_owned())), None);
        assert_eq!(bounded_version(Some(String::new())), None);
        assert_eq!(bounded_version(Some("9".repeat(33))), None);
        assert_eq!(bounded_version(None), None);
    }

    #[test]
    fn supported_paths_are_the_origin_and_api_only() {
        for path in ["", "/", "api", "/api"] {
            assert!(supported_path(path), "{path}");
        }
        for path in ["/session", "/api/session", "/not-api"] {
            assert!(!supported_path(path), "{path}");
        }
    }
}
