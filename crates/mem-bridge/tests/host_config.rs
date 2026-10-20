//! W5.1 step 11 — host-config oracle (`crates/mem-bridge/tests/host_config.rs`).
//!
//! The config surface parses from env with defaults (bridge-schema.md §9):
//! same names, same defaults, BOTH lanes. Two writers exist for the status
//! file — this oracle only pins that the ENGINE resolves them.

use std::path::PathBuf;

use mem_bridge::engine::{DEFAULT_WEB_HOST, DEFAULT_WEB_PORT, HostConfig, LaunchMode};

#[test]
fn test_host_config_defaults() {
    // The default constructor reads NO env var (defaults are STATIC in §9).
    let config = HostConfig::default();
    assert_eq!(
        config.storage_root,
        HostConfig::default().storage_root,
        "the default constructor is deterministic"
    );
    assert_eq!(
        config.mode,
        LaunchMode::Stdio,
        "OPENCODE_MEM_MODE defaults to stdio (the wrapper's child)"
    );
    assert_eq!(
        config.web_host, DEFAULT_WEB_HOST,
        "the axum bind host defaults to loopback (D-11 posture)"
    );
    assert!(
        config.web_host.parse::<std::net::IpAddr>().is_ok(),
        "the default host is an IP literal, not a name"
    );
    assert_eq!(
        config.web_port, DEFAULT_WEB_PORT,
        "3742 is the default bind port"
    );
    let root = config.storage_root.to_string_lossy().into_owned();
    assert!(
        root.ends_with("/.opencode-mem/data"),
        "the default root is [$HOME]/.opencode-mem/data (mem_core DEFAULTS): {root}"
    );
    assert!(
        PathBuf::from(&root).is_absolute(),
        "the default root is absolute: {root}"
    );
}

#[test]
fn test_host_config_from_env() {
    let root = format!(
        "{}/mem-bridge-host-config-env-root",
        std::env::temp_dir().display()
    );
    // SAFETY: this process is a single-threaded test binary; env mutation
    // is safe when no other thread reads it (only the two host_config
    // tests read env here).
    unsafe {
        std::env::set_var("OPENCODE_MEM_STORAGE_ROOT", &root);
        std::env::set_var("OPENCODE_MEM_MODE", "standalone");
        std::env::set_var("OPENCODE_MEM_WEB_HOST", "127.0.0.2");
        std::env::set_var("OPENCODE_MEM_WEB_PORT", "4901");
    }

    let config = HostConfig::from_env();
    assert_eq!(
        config.storage_root,
        PathBuf::from(root.as_str()),
        "the root env var wins over the default"
    );
    assert_eq!(config.mode, LaunchMode::Standalone);
    assert_eq!(config.web_host, "127.0.0.2");
    assert_eq!(config.web_port, 4901);

    // Blanking the values falls BACK to defaults (fresh env semantics).
    unsafe {
        std::env::remove_var("OPENCODE_MEM_MODE");
        std::env::set_var("OPENCODE_MEM_WEB_PORT", "not-a-port");
    }
    let config = HostConfig::from_env();
    assert_eq!(config.mode, LaunchMode::Stdio);
    assert_eq!(
        config.web_port, DEFAULT_WEB_PORT,
        "a non-numeric port never crashes the parse"
    );

    unsafe {
        std::env::remove_var("OPENCODE_MEM_STORAGE_ROOT");
        std::env::remove_var("OPENCODE_MEM_WEB_HOST");
        std::env::remove_var("OPENCODE_MEM_WEB_PORT");
    }
    let config = HostConfig::from_env();
    assert_eq!(
        config.storage_root,
        HostConfig::default().storage_root,
        "an unset env lands on the defaults"
    );
}
