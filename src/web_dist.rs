//! Where the composition root finds the built web UI (`web-ui/dist`).
//!
//! Upstream `web-server.ts:838-846` probes two `__dirname`-relative candidates
//! and takes the first that EXISTS. This module mirrors that shape, adds the
//! readiness marker (`index.html` as a regular file) and an explicit
//! env/CLI override, and returns a value the launcher can either hand to
//! mem-web or turn into a loud, diagnosable refusal (W6.8).
//!
//! Design notes that the call sites depend on:
//!
//! - The setting is **composition-root-local**. It is deliberately NOT a
//!   `mem_core::Config` field: upstream has no `webDistDir`, the location is a
//!   deployment concern, and the config file must never be able to redirect
//!   the static host.
//! - Precedence is CLI flag > env var > runtime-derived default, decided by
//!   [`explicit_setting`] and passed into [`resolve`] as one value.
//! - [`resolve`] performs only the readiness probe (an `exists`/`is_file`
//!   check on `<candidate>/index.html`); it reads no environment variable.
//! - The diagnostic payload is built from [`ResolvedDist::lossy_candidates`]
//!   (`Vec<String>`) and [`ResolvedDist::error_message`] (`Path::display()`),
//!   never from a `Vec<PathBuf>`, so serializing it cannot panic on a
//!   non-UTF-8 path.

use std::path::{Path, PathBuf};

use mem_store::serde_json::{Value, json};

/// The env var naming an explicit dist directory; the plugin wrapper forwards it
/// (issue 36) in the child environment and as `--web-dist`.
pub const ENV_VAR: &str = "OPENCODE_MEM_WEB_DIST";

/// The CLI flag naming an explicit dist directory (`--web-dist <path>` or
/// `--web-dist=<path>`); it wins over [`ENV_VAR`].
pub const CLI_FLAG: &str = "--web-dist";

/// The readiness marker: a candidate is a usable build only when this entry
/// exists as a regular file.
const INDEX_HTML: &str = "index.html";

/// A resolved dist location: either a usable build or the concrete candidates
/// that were tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDist {
    /// A usable build (the directory holds `index.html`).
    Ready { dir: PathBuf },
    /// No usable build; `candidates` is everything that was probed (in
    /// order) and `explicit` is set when the caller configured the path.
    Missing {
        candidates: Vec<PathBuf>,
        explicit: Option<PathBuf>,
    },
}

impl ResolvedDist {
    /// The usable directory, if any.
    pub fn dir(&self) -> Option<&Path> {
        match self {
            Self::Ready { dir } => Some(dir),
            Self::Missing { .. } => None,
        }
    }

    /// Every candidate that was probed, in probe order.
    pub fn candidates(&self) -> &[PathBuf] {
        match self {
            Self::Ready { dir } => std::slice::from_ref(dir),
            Self::Missing { candidates, .. } => candidates,
        }
    }

    /// The candidates as lossy strings — the panic-free form the diagnostic
    /// payload uses (a `PathBuf` in `json!` would panic on non-UTF-8 bytes).
    pub fn lossy_candidates(&self) -> Vec<String> {
        self.candidates()
            .iter()
            .map(|candidate| candidate.to_string_lossy().into_owned())
            .collect()
    }

    /// The human-readable reason the launch is (or would be) refused.
    ///
    /// Built with `Path::display()`, so it is total for any filesystem path.
    pub fn error_message(&self) -> String {
        match self {
            Self::Ready { dir } => format!("web dist ready at {}", dir.display()),
            Self::Missing {
                candidates,
                explicit,
            } => {
                let tried = candidates
                    .iter()
                    .map(|candidate| candidate.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                match explicit {
                    Some(path) => format!(
                        "web dist {} does not exist or has no {INDEX_HTML} file \
                         (from {ENV_VAR}/{CLI_FLAG}); tried: {tried}",
                        path.display()
                    ),
                    None => format!(
                        "no web dist found; tried: {tried} \
                         (set {ENV_VAR} or pass {CLI_FLAG} to override)"
                    ),
                }
            }
        }
    }

    /// The structured diagnostic payload for the `web.dist_missing` event.
    ///
    /// `Ready` yields `Null` (a usable build never emits the event), which
    /// makes the method total and independently testable. `src/main.rs` passes
    /// this value verbatim to `structured_stderr` and never builds the JSON
    /// inline.
    pub fn missing_payload(&self) -> Value {
        match self {
            Self::Ready { .. } => Value::Null,
            Self::Missing { .. } => json!({
                "dist_candidates": self.lossy_candidates(),
                "reason": self.error_message(),
            }),
        }
    }
}

/// Applies the CLI-over-env precedence.
///
/// Empty and whitespace-only values are dropped exactly like an unset name, so
/// `--web-dist=` falls through to the env var (and then the default) instead
/// of resolving to the process cwd.
pub fn explicit_setting(cli: Option<&Path>, env: Option<&Path>) -> Option<PathBuf> {
    fn usable(value: Option<&Path>) -> Option<PathBuf> {
        value
            .filter(|path| !path.as_os_str().to_string_lossy().trim().is_empty())
            .map(Path::to_path_buf)
    }
    usable(cli).or_else(|| usable(env))
}

/// The two exe-relative default candidates, in probe order.
///
/// 1. `exe_dir/web-ui/dist` — the PACKAGED layout (the bundle staged beside
///    the engine binary).
/// 2. `exe_dir/../../web-ui/dist` — the CARGO-CHECKOUT dev layout (the binary
///    lives at `target/<profile>/opencode-agents-memory`, so `../..` is the
///    checkout root).
///
/// Nothing here is hard-coded to an absolute or cwd-relative path: the same
/// binary resolves the same way wherever it is installed.
pub fn default_candidates(exe_dir: &Path) -> Vec<PathBuf> {
    vec![
        exe_dir.join("web-ui/dist"),
        exe_dir.join("../../web-ui/dist"),
    ]
}

/// Resolves the dist directory from the explicit setting and the runtime
/// layout.
///
/// An explicit path is absolutized against `cwd` (matching
/// `web-ui/scripts/serve-dist.mjs`'s `resolve(root)`), so both the served root
/// and the diagnostic name the same unambiguous directory. The first candidate
/// whose `index.html` is a regular file wins.
pub fn resolve(explicit: Option<&Path>, exe_dir: &Path, cwd: &Path) -> ResolvedDist {
    let candidates = match explicit {
        Some(path) => vec![absolutize(path, cwd)],
        None => default_candidates(exe_dir),
    };
    if let Some(dir) = candidates.iter().find(|candidate| is_ready(candidate)) {
        return ResolvedDist::Ready { dir: dir.clone() };
    }
    ResolvedDist::Missing {
        candidates,
        explicit: explicit.map(|path| absolutize(path, cwd)),
    }
}

/// Resolves `path` against `cwd` unless it is already absolute.
fn absolutize(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Whether `dir` is a usable build: `<dir>/index.html` is a regular file.
///
/// `is_file()` (not `exists()`) rejects a directory named `index.html`.
fn is_ready(dir: &Path) -> bool {
    dir.join(INDEX_HTML).is_file()
}
