//! Git probing seam (upstream `src/services/tags.ts` module-level
//! `runGit` + the `process.env.PATH` trusted-command scan).
//!
//! This is the deep seam of the tags service: production uses
//! [`SystemGitProbe`] (W1.1 Step 6); tests inject fakes so no test spawns a
//! git process by accident. Each trait method translates one upstream
//! function:
//!
//! - `common_dir` → `getGitCommonDir` (`rev-parse --git-common-dir`, with
//!   the relative-output resolution and `realpathSync`)
//! - `top_level` → `getGitTopLevel` (`rev-parse --show-toplevel`)
//! - `remote_origin_url` → `getGitRepoUrl` (`config --get remote.origin.url`)
//! - `user_email` → `getGitEmail` (`config user.email`)
//! - `user_name` → `getGitName` (`config user.name`)

use std::path::PathBuf;

use crate::env::EnvSource;

pub trait GitProbe {
    fn common_dir(&self, directory: &std::path::Path) -> Option<PathBuf>;
    fn top_level(&self, directory: &std::path::Path) -> Option<PathBuf>;
    fn remote_origin_url(&self, directory: &std::path::Path) -> Option<String>;
    fn user_email(&self, directory: &std::path::Path) -> Option<String>;
    fn user_name(&self, directory: &std::path::Path) -> Option<String>;
}

/// The production [`GitProbe`] over the real `git` binary (upstream
/// `runGit` + `resolveTrustedGitCommand`).
///
/// Anti-shim containment: a `git` executable whose canonical path lies
/// inside the untrusted project root (the nearest ancestor containing
/// `.git` or the project marker) is NEVER run — a checked-out tree can
/// ship a malicious `git` shim, and running it would be code execution
/// from repository content. Candidates are canonicalized before the
/// containment check so symlinks cannot smuggle one past it.
///
/// The probe never panics: any spawn/canonicalize/IO failure collapses to
/// `None` / a skipped candidate.
pub struct SystemGitProbe<'a> {
    env: &'a dyn EnvSource,
}

impl<'a> SystemGitProbe<'a> {
    /// `env` supplies `PATH` (and the Windows shell variables); production
    /// passes [`crate::env::ProcessEnv`], tests inject maps.
    pub fn new(env: &'a dyn EnvSource) -> Self {
        Self { env }
    }

    /// Upstream `runGit`: resolve a trusted `git`, then run it with
    /// `stdio: ["ignore", "pipe", "ignore"]`; empty output, non-zero exit
    /// or any spawn failure → `None`. Hermeticity hardening (documented
    /// divergence — identical behavior when unset, the production case):
    /// the `GIT_DIR` family is removed from the child so an ambient
    /// variable can never redirect the probe away from `directory`.
    fn run_git(&self, directory: &std::path::Path, args: &[&str]) -> Option<String> {
        let resolved = self.resolve_trusted_git(directory)?;
        let mut output = match resolved.shell {
            #[cfg(windows)]
            Some(shell) => {
                let mut cmd = std::process::Command::new(&shell);
                cmd.arg("/d").arg("/s").arg("/c").arg(&resolved.executable);
                cmd
            }
            #[cfg(unix)]
            None => std::process::Command::new(&resolved.executable),
            _ => return None,
        };
        output
            .args(args)
            .current_dir(directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        for variable in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
        ] {
            output.env_remove(variable);
        }
        let output = output.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() { None } else { Some(text) }
    }

    /// Upstream `resolveTrustedGitCommand`: scan `PATH` for an executable
    /// `git` whose canonical path is not inside the untrusted project root.
    fn resolve_trusted_git(&self, directory: &std::path::Path) -> Option<ResolvedGit> {
        let untrusted_root = find_untrusted_project_root(directory);
        let executable_names: &[&str] = if cfg!(windows) {
            &["git.exe", "git.cmd", "git.bat"]
        } else {
            &["git"]
        };
        let path = self.env.get("PATH")?;
        let delimiter = if cfg!(windows) { ';' } else { ':' };

        for raw_entry in path.split(delimiter) {
            let entry = strip_surrounding_quotes(raw_entry.trim());
            if entry.is_empty() || !std::path::Path::new(entry).is_absolute() {
                continue;
            }
            for executable_name in executable_names {
                let candidate = std::path::Path::new(entry).join(executable_name);
                if !is_executable(&candidate) {
                    continue;
                }
                let canonical = match std::fs::canonicalize(&candidate) {
                    Ok(canonical) => canonical,
                    Err(_) => continue,
                };
                if is_path_inside(&untrusted_root, &canonical) {
                    continue;
                }

                #[cfg(unix)]
                {
                    let _ = executable_name;
                    return Some(ResolvedGit {
                        executable: canonical,
                        shell: None,
                    });
                }
                #[cfg(windows)]
                {
                    if *executable_name == "git.exe" {
                        return Some(ResolvedGit {
                            executable: canonical,
                            shell: None,
                        });
                    }
                    // Best-effort, UNVERIFIED HERE (no win32 runner; the
                    // three win32 upstream tests are recorded N-A):
                    // `git.cmd`/`git.bat` must run through a trusted shell.
                    let shell = resolve_trusted_windows_shell(self.env, &untrusted_root)?;
                    return Some(ResolvedGit {
                        executable: canonical,
                        shell: Some(shell),
                    });
                }
            }
        }
        None
    }
}

struct ResolvedGit {
    executable: PathBuf,
    shell: Option<PathBuf>,
}

impl GitProbe for SystemGitProbe<'_> {
    /// Upstream `getGitCommonDir` (`rev-parse --git-common-dir`): a
    /// relative output is resolved against `directory`; an existing result
    /// is canonicalized (upstream `realpathSync`).
    fn common_dir(&self, directory: &std::path::Path) -> Option<PathBuf> {
        let common_dir = self.run_git(directory, &["rev-parse", "--git-common-dir"])?;
        let resolved = if std::path::Path::new(&common_dir).is_absolute() {
            PathBuf::from(&common_dir)
        } else {
            directory.join(&common_dir)
        };
        if resolved.exists() {
            return Some(std::fs::canonicalize(&resolved).unwrap_or(resolved));
        }
        Some(resolved)
    }

    fn top_level(&self, directory: &std::path::Path) -> Option<PathBuf> {
        self.run_git(directory, &["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
    }

    fn remote_origin_url(&self, directory: &std::path::Path) -> Option<String> {
        self.run_git(directory, &["config", "--get", "remote.origin.url"])
    }

    fn user_email(&self, directory: &std::path::Path) -> Option<String> {
        self.run_git(directory, &["config", "user.email"])
    }

    fn user_name(&self, directory: &std::path::Path) -> Option<String> {
        self.run_git(directory, &["config", "user.name"])
    }
}

/// Surrounding double quotes from a trimmed PATH entry (upstream
/// `.replace(/^"(.*)"$/, "$1")`).
fn strip_surrounding_quotes(entry: &str) -> &str {
    if entry.len() >= 2 && entry.starts_with('"') && entry.ends_with('"') {
        &entry[1..entry.len() - 1]
    } else {
        entry
    }
}

/// Nearest ancestor of `directory` (inclusive) containing `.git` or the
/// project marker, else the canonical directory itself (upstream
/// `findUntrustedProjectRoot`) — the containment root for the trusted-git
/// scan.
fn find_untrusted_project_root(directory: &std::path::Path) -> PathBuf {
    let start = canonical_path(directory);
    let mut current = start.clone();
    loop {
        if current.join(".git").exists() || current.join(super::PROJECT_MARKER).exists() {
            return current;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return start,
        }
    }
}

/// `fs::canonicalize`, falling back to `std::path::absolute`, finally to
/// the raw path — never panics.
fn canonical_path(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// Lexical containment (upstream's `relative` + prefix check; the
/// `Path::starts_with` semantics are component-wise — equivalent for the
/// canonicalized inputs used here).
fn is_path_inside(root: &std::path::Path, candidate: &std::path::Path) -> bool {
    candidate.starts_with(root)
}

/// Executable check: metadata + owner/any execute bit (documented
/// approximation of upstream `accessSync(path, X_OK)`).
fn is_executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        // Best-effort on non-unix: the win32 branch additionally routes
        // .cmd/.bat through a trusted shell (see `run_git`).
        true
    }
}

#[cfg(windows)]
/// Best-effort, UNVERIFIED HERE (no win32 runner): locate `cmd.exe` outside
/// the untrusted root (upstream `resolveTrustedWindowsShell`).
fn resolve_trusted_windows_shell(
    env: &dyn EnvSource,
    untrusted_root: &std::path::Path,
) -> Option<PathBuf> {
    let system_root = env.get("SystemRoot");
    let candidates = [
        env.get("ComSpec"),
        system_root.map(|root| {
            std::path::Path::new(&root)
                .join("System32")
                .join("cmd.exe")
                .display()
                .to_string()
        }),
    ];
    for candidate in candidates.into_iter().flatten() {
        if !std::path::Path::new(&candidate).is_absolute() {
            continue;
        }
        let path = std::path::PathBuf::from(&candidate);
        if !is_executable(&path) {
            continue;
        }
        let canonical = match std::fs::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(_) => continue,
        };
        if is_path_inside(untrusted_root, &canonical) {
            continue;
        }
        return Some(canonical);
    }
    None
}
