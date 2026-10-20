//! The server bootstrap and lifecycle — the port of the `WebServer` startup
//! surface of upstream `web-server.ts`.
//!
//! Upstream's takeover / fallback-port cases are **N-A** per D-1 (the port
//! chooses fail-fast on a port conflict; there is no port hopping and no
//! takeover loop) — they get a registry row instead of a test.

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use mem_core::config::{Config, ConfigPaths};
use mem_web::WebError;
use mem_web::WebServerSettings;
use mem_web::server::{WebServerHandle, start};
use mem_web::web_auth::WebAuth;

/// The local token the shell would carry.
const LOCAL_TOKEN: &str = "local-tok-secret";

/// A dist directory that does not exist (D-7).
const MISSING_DIST: &str = "/nonexistent-mem-web-dist";

fn settings(host: &str, port: u16) -> WebServerSettings {
    WebServerSettings {
        enabled: true,
        host: host.to_owned(),
        port,
        auth_password: None,
        auth_username: None,
        api_token: None,
    }
}

fn state() -> mem_web::server::AppState {
    common::app_state(
        WebAuth::new(None, None, "osuser"),
        None,
        Some(LOCAL_TOKEN),
        PathBuf::from(MISSING_DIST),
    )
}

/// Reserves a free loopback port and releases it again.
fn reserve_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    let port = listener.local_addr().expect("read the bound port").port();
    drop(listener);
    port
}

#[tokio::test]
async fn test_start_returns_none_and_binds_nothing_when_disabled() {
    let port = reserve_port();
    let mut config = settings("127.0.0.1", port);
    config.enabled = false;

    let handle = start(&config, state())
        .await
        .expect("a disabled server must not fail");

    assert!(handle.is_none(), "a disabled server must not start");

    let probe = std::net::TcpListener::bind(("127.0.0.1", port))
        .expect("the reserved port must still be bindable");
    drop(probe);
}

#[tokio::test]
async fn test_start_fails_fast_when_port_in_use() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    let port = occupied.local_addr().expect("read the bound port").port();

    let error = start(&settings("127.0.0.1", port), state())
        .await
        .expect_err("a port conflict must fail fast");

    match error {
        WebError::AddrInUse {
            host,
            port: reported,
        } => {
            assert_eq!(host, "127.0.0.1");
            assert_eq!(reported, port);
        }
        other => panic!("expected AddrInUse, got {other:?}"),
    }
    drop(occupied);
}

#[tokio::test]
async fn test_start_refuses_non_loopback_host_without_auth_before_binding() {
    let port = reserve_port();

    let error = start(&settings("0.0.0.0", port), state())
        .await
        .expect_err("a network bind without a credential must be refused");

    match error {
        WebError::NetworkAuthRequired { host } => assert_eq!(host, "0.0.0.0"),
        other => panic!("expected NetworkAuthRequired, got {other:?}"),
    }

    let probe = std::net::TcpListener::bind(("127.0.0.1", port))
        .expect("the refused bind must not have created a listener");
    drop(probe);
}

#[tokio::test]
async fn test_start_binds_ephemeral_port_and_serves_health() {
    let handle = common::spawn_server(&settings("127.0.0.1", 0), state()).await;
    let addr = handle.local_addr();
    assert_ne!(addr.port(), 0, "an ephemeral port must be assigned");

    let raw = format!(
        "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        addr.port()
    );
    let response = tokio::task::spawn_blocking(move || common::raw_request(addr, &raw))
        .await
        .expect("the socket helper must not panic");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_string(),
        r#"{"success":true,"status":"ok","authEnabled":false}"#
    );

    handle.stop().await.expect("the server must stop cleanly");
}

#[tokio::test]
async fn test_stop_releases_port_for_rebind() {
    let handle = common::spawn_server(&settings("127.0.0.1", 0), state()).await;
    let addr = handle.local_addr();

    handle.stop().await.expect("the server must stop cleanly");

    let rebound = std::net::TcpListener::bind(addr).expect("the port must be free again");
    drop(rebound);
}

#[tokio::test]
async fn test_foreign_host_header_is_refused_over_tcp_with_no_token_in_body() {
    let handle = common::spawn_server(&settings("127.0.0.1", 0), state()).await;
    let addr = handle.local_addr();
    let port = addr.port();

    // A foreign Host (the DNS-rebinding shape) is refused.
    let foreign = format!("GET /api/health HTTP/1.1\r\nHost: evil.example:{port}\r\n\r\n");
    let foreign_addr = addr;
    let response = tokio::task::spawn_blocking(move || common::raw_request(foreign_addr, &foreign))
        .await
        .expect("the socket helper must not panic");
    assert_eq!(response.status, 403);
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Host header is not allowed."}"#
    );
    assert!(!response.body_string().contains("__OPENCODE_MEM_TOKEN__"));
    assert!(!response.body_string().contains(LOCAL_TOKEN));

    // The helper-injected Host keeps the project's own traffic alive.
    let allowed = "GET /api/health HTTP/1.1\r\n\r\n".to_owned();
    let allowed_addr = addr;
    let response = tokio::task::spawn_blocking(move || common::raw_request(allowed_addr, &allowed))
        .await
        .expect("the socket helper must not panic");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_string(),
        r#"{"success":true,"status":"ok","authEnabled":false}"#
    );

    handle.stop().await.expect("the server must stop cleanly");
}

#[test]
fn test_settings_from_config_copies_web_server_fields() {
    // The hermetic load path of `crates/mem-core/tests/config_defaults.rs`:
    // an empty temp home plus an injected env map.
    let home = common::temp_dir_unique("settings-defaults");
    let paths = ConfigPaths::standard(home.clone());
    let config = Config::load(&paths, &BTreeMap::new()).expect("the defaults must load");

    let settings = WebServerSettings::from_config(&config);
    assert!(settings.enabled);
    assert_eq!(settings.port, 4747);
    assert_eq!(settings.host, "127.0.0.1");
    assert_eq!(settings.auth_password, None);
    assert_eq!(settings.auth_username, None);
    assert_eq!(settings.api_token, None);

    // The configured values are copied verbatim.
    let config_dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&config_dir).expect("create the config dir");
    std::fs::write(
        config_dir.join("opencode-mem.jsonc"),
        r#"{
            "webServerEnabled": true,
            "webServerPort": 5555,
            "webServerHost": "192.168.1.10",
            "webServerAuthUsername": "admin",
            "webServerAuthPassword": "secret",
            "webServerApiToken": "api-token"
        }"#,
    )
    .expect("write the config file");
    let config = Config::load(&paths, &BTreeMap::new()).expect("the config must load");

    let settings = WebServerSettings::from_config(&config);
    assert!(settings.enabled);
    assert_eq!(settings.port, 5555);
    assert_eq!(settings.host, "192.168.1.10");
    assert_eq!(settings.auth_username.as_deref(), Some("admin"));
    assert_eq!(settings.auth_password.as_deref(), Some("secret"));
    assert_eq!(settings.api_token.as_deref(), Some("api-token"));
}

/// The handle type is part of the public surface.
#[allow(dead_code)]
fn assert_handle_type(_handle: &WebServerHandle, _addr: SocketAddr) {}
