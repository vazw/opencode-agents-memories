//! Environment seam.
//!
//! Tests never mutate the process environment (it races under parallel test
//! execution); they inject plain maps instead. Production call sites pass
//! [`ProcessEnv`].

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Read-only environment lookup.
pub trait EnvSource {
    fn get(&self, key: &str) -> Option<String>;
}

/// Reads the real process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

impl EnvSource for BTreeMap<String, String> {
    fn get(&self, key: &str) -> Option<String> {
        BTreeMap::get(self, key).cloned()
    }
}

impl EnvSource for HashMap<String, String> {
    fn get(&self, key: &str) -> Option<String> {
        HashMap::get(self, key).cloned()
    }
}

/// Resolves the user home directory: first non-empty of `HOME`,
/// `USERPROFILE` (upstream `os.homedir()` semantics via env).
pub fn home_dir(env: &dyn EnvSource) -> Option<PathBuf> {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = env.get(key).filter(|value| !value.is_empty()) {
            return Some(PathBuf::from(value));
        }
    }
    None
}
