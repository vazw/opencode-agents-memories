//! Host endpoint discovery matrix — both resolution branches.
//!
//! Every rule is pinned by its own fixture: a `service.json` under a temp
//! state dir, an injected env map, an injected liveness probe and an injected
//! process clock. No process environment is mutated, no network is touched.
//!
//! Branch A = a non-blank `opencodeBaseUrl` override (validation + credential
//! binding only, no liveness probe). Branch B = the service file is the
//! authority.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use common::TestDir;
use mem_ai::provider::{HostMethod, HostRequest};
use mem_bridge::host_endpoint::{
    HostWiringError, HostWiringRequest, ProcessClock, resolve_host_wiring,
};
use mem_store::platform::ProcessProbe;

/// Liveness oracle: only the pids listed here answer alive.
struct FakeProbe {
    alive: BTreeMap<u32, bool>,
}

impl FakeProbe {
    fn with(alive: &[u32]) -> Self {
        let mut map = BTreeMap::new();
        for pid in alive {
            map.insert(*pid, true);
        }
        Self { alive: map }
    }

    fn dead() -> Self {
        Self {
            alive: BTreeMap::new(),
        }
    }
}

impl ProcessProbe for FakeProbe {
    fn is_alive(&self, pid: u32) -> bool {
        self.alive.get(&pid).copied().unwrap_or(false)
    }
}

/// Start-time oracle keyed by pid (the fake clock).
struct FakeClock {
    starts: BTreeMap<u32, u64>,
}

impl FakeClock {
    fn with(starts: &[(u32, u64)]) -> Self {
        Self {
            starts: starts.iter().copied().collect(),
        }
    }

    fn empty() -> Self {
        Self {
            starts: BTreeMap::new(),
        }
    }
}

impl ProcessClock for FakeClock {
    fn start_time(&self, pid: u32) -> Option<u64> {
        self.starts.get(&pid).copied()
    }
}

/// A `service.json` body with the measured live shape.
fn service_json(port: u16, pid: u32, password: &str, version: &str) -> String {
    format!(
        r#"{{"id":"742f10c4-0e0a-4c2a-9c6e-5f0a0c1d2e3f","url":"http://127.0.0.1:{port}","pid":{pid},"password":"{password}","version":"{version}"}}"#
    )
}

/// Writes `<state>/opencode/service.json`.
fn write_service(state: &Path, body: &str) {
    let dir = state.join("opencode");
    std::fs::create_dir_all(&dir).expect("create the opencode state dir");
    std::fs::write(dir.join("service.json"), body).expect("write service.json");
}

fn env_with_xdg(state: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("XDG_STATE_HOME".to_string(), state.display().to_string());
    env
}

fn env_with_home(home: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("HOME".to_string(), home.display().to_string());
    env
}

fn resolve(
    env: &BTreeMap<String, String>,
    probe: &FakeProbe,
    clock: &FakeClock,
    request: &HostWiringRequest<'_>,
) -> Result<mem_bridge::host_endpoint::HostWiring, HostWiringError> {
    resolve_host_wiring(env, probe, clock, request)
}

fn live_request<'a>() -> HostWiringRequest<'a> {
    HostWiringRequest {
        override_url: None,
        configured_provider: Some("opencode-go"),
        configured_model: Some("beam-free"),
    }
}

#[test]
fn test_service_file_origin_and_api_prefix_are_used() {
    let dir = TestDir::new("host-endpoint-happy");
    write_service(dir.path(), &service_json(49374, 4242, "s3cret", "2.2.5"));

    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::with(&[(4242, 100)]),
        &live_request(),
    )
    .expect("a live, well-formed service file resolves");

    assert_eq!(wiring.base_url, "http://127.0.0.1:49374/api");
    assert_eq!(wiring.provider_id, "opencode-go");
    assert_eq!(wiring.model_id, "beam-free");
    assert_eq!(wiring.host_version.as_deref(), Some("2.2.5"));
    let authorization = wiring.authorization.expect("credentials attach");
    assert_eq!(
        authorization.header_value().as_str(),
        "Basic b3BlbmNvZGU6czNjcmV0"
    );
}

#[test]
fn test_service_file_ignores_unknown_keys() {
    let dir = TestDir::new("host-endpoint-unknown-keys");
    // The measured file carries an `id` the resolver does not model.
    write_service(
        dir.path(),
        r#"{"id":"742f10c4","url":"http://127.0.0.1:49374","pid":4242,"password":"s3cret","custom":{"nested":true}}"#,
    );

    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect("unknown keys are ignored, never a parse failure");
    assert_eq!(wiring.base_url, "http://127.0.0.1:49374/api");
    assert_eq!(wiring.host_version, None);
}

#[test]
fn test_xdg_state_home_takes_precedence_over_home() {
    let xdg = TestDir::new("host-endpoint-xdg");
    write_service(xdg.path(), &service_json(5001, 4242, "s3cret", "2.2.5"));
    let home = TestDir::new("host-endpoint-home");
    write_service(
        &home.path().join(".local").join("state"),
        &service_json(5002, 4242, "s3cret", "2.2.5"),
    );

    let mut env = env_with_xdg(xdg.path());
    env.insert("HOME".to_string(), home.path().display().to_string());

    let wiring = resolve(
        &env,
        &FakeProbe::with(&[4242]),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect("XDG_STATE_HOME wins");
    assert_eq!(wiring.base_url, "http://127.0.0.1:5001/api");
}

#[test]
fn test_home_local_state_is_the_fallback_state_dir() {
    let home = TestDir::new("host-endpoint-home-only");
    write_service(
        &home.path().join(".local").join("state"),
        &service_json(5002, 4242, "s3cret", "2.2.5"),
    );

    let wiring = resolve(
        &env_with_home(home.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect("$HOME/.local/state is the fallback");
    assert_eq!(wiring.base_url, "http://127.0.0.1:5002/api");
}

#[test]
fn test_missing_state_dir_reports_state_dir_unknown() {
    let error = resolve(
        &BTreeMap::new(),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect_err("no XDG_STATE_HOME and no HOME is a resolution failure");
    assert_eq!(error, HostWiringError::StateDirUnknown);
    assert_eq!(error.reason(), "state_dir_unknown");
}

#[test]
fn test_missing_service_file_reports_service_file_missing() {
    let dir = TestDir::new("host-endpoint-missing-file");
    let error = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect_err("no service file, no override");
    assert_eq!(error.reason(), "service_file_missing");
}

#[test]
fn test_unreadable_service_file_reports_unreadable() {
    let dir = TestDir::new("host-endpoint-unreadable");
    let path = dir.path().join("opencode").join("service.json");
    std::fs::create_dir_all(&path).expect("a DIRECTORY where the file belongs");

    let error = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect_err("EISDIR is unreadable, never missing");
    assert_eq!(error.reason(), "service_file_unreadable");
}

#[test]
fn test_malformed_service_file_reports_service_file_malformed() {
    for (label, body) in [
        ("not json", "not json"),
        (
            "no pid",
            r#"{"url":"http://127.0.0.1:49374","password":"s3cret"}"#,
        ),
        (
            "pid zero",
            r#"{"url":"http://127.0.0.1:49374","pid":0,"password":"s3cret"}"#,
        ),
        (
            "no password",
            r#"{"url":"http://127.0.0.1:49374","pid":4242}"#,
        ),
    ] {
        let dir = TestDir::new("host-endpoint-malformed");
        write_service(dir.path(), body);
        let error = resolve(
            &env_with_xdg(dir.path()),
            &FakeProbe::with(&[4242]),
            &FakeClock::empty(),
            &live_request(),
        )
        .expect_err("malformed input never fabricates a URL");
        assert_eq!(
            error.reason(),
            "service_file_malformed",
            "case: {label} must be service_file_malformed, got {error:?}"
        );
    }
}

#[test]
fn test_dead_pid_is_rejected() {
    let dir = TestDir::new("host-endpoint-dead-pid");
    write_service(dir.path(), &service_json(49374, 4242, "s3cret", "2.2.5"));

    let error = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect_err("a dead pid is not the live host");
    assert_eq!(error.reason(), "host_pid_dead");

    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect("an alive pid resolves");
    assert_eq!(wiring.base_url, "http://127.0.0.1:49374/api");
}

#[test]
fn test_recycled_pid_is_rejected() {
    let dir = TestDir::new("host-endpoint-recycled");
    write_service(dir.path(), &service_json(49374, 4242, "s3cret", "2.2.5"));

    // The recorded pid started AFTER the engine ⇒ it cannot be the host the
    // engine was spawned by, so the pid was recycled.
    let engine = std::process::id();
    let error = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::with(&[(4242, 900), (engine, 800)]),
        &live_request(),
    )
    .expect_err("a later start time than the engine is provably recycled");
    assert_eq!(error.reason(), "host_pid_recycled");

    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::with(&[(4242, 700), (engine, 800)]),
        &live_request(),
    )
    .expect("an earlier start time is the host that spawned the engine");
    assert_eq!(wiring.base_url, "http://127.0.0.1:49374/api");
}

#[test]
fn test_non_loopback_url_is_rejected() {
    for (label, url) in [
        ("private address", "http://192.168.1.10:49374"),
        ("userinfo smuggle", "http://127.0.0.1:49374@evil.example:80"),
    ] {
        let dir = TestDir::new("host-endpoint-not-loopback");
        write_service(
            dir.path(),
            &format!(r#"{{"url":"{url}","pid":4242,"password":"s3cret"}}"#),
        );
        let error = resolve(
            &env_with_xdg(dir.path()),
            &FakeProbe::with(&[4242]),
            &FakeClock::empty(),
            &live_request(),
        )
        .expect_err("loopback only");
        assert_eq!(
            error.reason(),
            "host_url_not_loopback",
            "case: {label}; got {error:?}"
        );
    }
}

#[test]
fn test_relative_and_non_http_urls_report_unsupported() {
    for (label, url) in [
        ("file scheme", "file:///tmp/x"),
        ("ftp scheme", "ftp://127.0.0.1:21"),
        ("relative path", "/session"),
        ("missing port", "http://127.0.0.1"),
    ] {
        let dir = TestDir::new("host-endpoint-unsupported");
        write_service(
            dir.path(),
            &format!(r#"{{"url":"{url}","pid":4242,"password":"s3cret"}}"#),
        );
        let error = resolve(
            &env_with_xdg(dir.path()),
            &FakeProbe::with(&[4242]),
            &FakeClock::empty(),
            &live_request(),
        )
        .expect_err("only an http(s) loopback origin with an explicit port is supported");
        assert_eq!(
            error.reason(),
            "host_url_unsupported",
            "case: {label}; got {error:?}"
        );
    }
}

#[test]
fn test_file_branch_path_is_unsupported() {
    let dir = TestDir::new("host-endpoint-file-path");
    write_service(
        dir.path(),
        r#"{"url":"http://127.0.0.1:49374/session","pid":4242,"password":"s3cret"}"#,
    );
    let error = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect_err("the host wrote a sub-path: never silently stripped or doubled");
    assert_eq!(error.reason(), "host_url_unsupported");
}

#[test]
fn test_override_path_reports_host_override_invalid() {
    let dir = TestDir::new("host-endpoint-override-path");
    let request = HostWiringRequest {
        override_url: Some("http://127.0.0.1:5000/not-api"),
        ..live_request()
    };
    let error = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &request,
    )
    .expect_err("the operator named an unusable base");
    assert_eq!(error.reason(), "host_override_invalid");
}

#[test]
fn test_override_wins_over_a_live_service_file() {
    let dir = TestDir::new("host-endpoint-override-wins");
    write_service(dir.path(), &service_json(49374, 4242, "s3cret", "2.2.5"));
    let request = HostWiringRequest {
        override_url: Some("http://127.0.0.1:5000"),
        ..live_request()
    };

    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::empty(),
        &request,
    )
    .expect("the override is the authority");
    assert_eq!(wiring.base_url, "http://127.0.0.1:5000/api");
    // A different origin gets NO credentials: the override cannot inject them.
    assert!(wiring.authorization.is_none());
}

#[test]
fn test_blank_override_falls_through_to_the_service_file() {
    let dir = TestDir::new("host-endpoint-blank-override");
    write_service(dir.path(), &service_json(49374, 4242, "s3cret", "2.2.5"));
    let request = HostWiringRequest {
        override_url: Some("   "),
        ..live_request()
    };

    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::empty(),
        &request,
    )
    .expect("a blank override is unset, so the file is the authority");
    assert_eq!(wiring.base_url, "http://127.0.0.1:49374/api");
}

#[test]
fn test_non_loopback_override_is_rejected() {
    let dir = TestDir::new("host-endpoint-override-not-loopback");
    write_service(dir.path(), &service_json(49374, 4242, "s3cret", "2.2.5"));
    let request = HostWiringRequest {
        override_url: Some("http://10.0.0.5:8080"),
        ..live_request()
    };

    let error = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::empty(),
        &request,
    )
    .expect_err("an invalid override never falls back to the file");
    assert_eq!(error, HostWiringError::UrlNotLoopback);
    assert_eq!(error.reason(), "host_url_not_loopback");
}

#[test]
fn test_override_already_carrying_the_api_path_is_not_doubled() {
    let dir = TestDir::new("host-endpoint-override-api-path");
    let request = HostWiringRequest {
        override_url: Some("http://127.0.0.1:5000/api"),
        ..live_request()
    };
    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &request,
    )
    .expect("the /api path is accepted, not doubled");
    assert_eq!(wiring.base_url, "http://127.0.0.1:5000/api");
}

#[test]
fn test_override_without_a_service_file_succeeds_without_credentials() {
    let dir = TestDir::new("host-endpoint-override-no-file");
    let request = HostWiringRequest {
        override_url: Some("http://127.0.0.1:5000"),
        ..live_request()
    };
    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &request,
    )
    .expect("the override resolves on its own");
    assert_eq!(wiring.base_url, "http://127.0.0.1:5000/api");
    assert!(wiring.authorization.is_none());
}

#[test]
fn test_override_without_a_state_dir_succeeds_without_credentials() {
    let request = HostWiringRequest {
        override_url: Some("http://127.0.0.1:5000"),
        ..live_request()
    };
    let wiring = resolve(
        &BTreeMap::new(),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &request,
    )
    .expect("no state dir is not a failure on branch A");
    assert_eq!(wiring.base_url, "http://127.0.0.1:5000/api");
    assert!(wiring.authorization.is_none());
}

#[test]
fn test_override_with_a_malformed_service_file_fails_closed() {
    let dir = TestDir::new("host-endpoint-override-malformed");
    write_service(dir.path(), "not json");
    let request = HostWiringRequest {
        override_url: Some("http://127.0.0.1:5000"),
        ..live_request()
    };
    let error = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &request,
    )
    .expect_err("a credential source that cannot be parsed is named, not ignored");
    assert_eq!(error.reason(), "service_file_malformed");
}

#[test]
fn test_service_credentials_are_attached_for_the_same_origin_only() {
    let dir = TestDir::new("host-endpoint-credential-binding");
    write_service(dir.path(), &service_json(5000, 4242, "s3cret", "2.2.5"));

    let same = HostWiringRequest {
        override_url: Some("http://127.0.0.1:5000"),
        ..live_request()
    };
    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &same,
    )
    .expect("same origin");
    assert!(
        wiring.authorization.is_some(),
        "a same-origin override binds the file's credentials"
    );

    let different = HostWiringRequest {
        override_url: Some("http://localhost:5000"),
        ..live_request()
    };
    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::dead(),
        &FakeClock::empty(),
        &different,
    )
    .expect("different origin");
    assert!(
        wiring.authorization.is_none(),
        "a different origin must not receive the file's credentials"
    );
}

#[test]
fn test_unset_or_inherit_model_reports_host_model_unresolved() {
    let dir = TestDir::new("host-endpoint-model-unresolved");
    write_service(dir.path(), &service_json(49374, 4242, "s3cret", "2.2.5"));

    for (label, model) in [("unset", None), ("inherit literal", Some("inherit"))] {
        let request = HostWiringRequest {
            configured_model: model,
            ..live_request()
        };
        let error = resolve(
            &env_with_xdg(dir.path()),
            &FakeProbe::with(&[4242]),
            &FakeClock::empty(),
            &request,
        )
        .expect_err("an unresolved model fails closed, never forwards the literal");
        assert_eq!(
            error.reason(),
            "host_model_unresolved",
            "case: {label}; got {error:?}"
        );
    }
}

/// The reason-pinning table: EVERY one of the ten reasons has exactly one
/// input that reaches it, and no two variants share a string.
#[test]
fn test_every_host_reason_has_its_pinning_input() {
    let valid = TestDir::new("host-endpoint-pin-valid");
    write_service(valid.path(), &service_json(49374, 4242, "s3cret", "2.2.5"));

    let unreadable = TestDir::new("host-endpoint-pin-unreadable");
    std::fs::create_dir_all(unreadable.path().join("opencode").join("service.json"))
        .expect("a directory where the file belongs");

    let malformed = TestDir::new("host-endpoint-pin-malformed");
    write_service(malformed.path(), "not json");

    let not_loopback = TestDir::new("host-endpoint-pin-not-loopback");
    write_service(
        not_loopback.path(),
        r#"{"url":"http://192.168.1.10:49374","pid":4242,"password":"s3cret"}"#,
    );

    let unsupported = TestDir::new("host-endpoint-pin-unsupported");
    write_service(
        unsupported.path(),
        r#"{"url":"file:///tmp/x","pid":4242,"password":"s3cret"}"#,
    );

    let empty = TestDir::new("host-endpoint-pin-empty");
    let no_state_dir = BTreeMap::new();
    let inherit = HostWiringRequest {
        configured_model: Some("inherit"),
        ..live_request()
    };
    let override_path = HostWiringRequest {
        override_url: Some("http://127.0.0.1:5000/not-api"),
        ..live_request()
    };

    let alive = FakeProbe::with(&[4242]);
    let dead = FakeProbe::dead();
    let no_clock = FakeClock::empty();
    let recycled = FakeClock::with(&[(4242, 900), (std::process::id(), 800)]);

    let cases: Vec<(&str, Result<_, HostWiringError>)> = vec![
        (
            "host_override_invalid",
            resolve(
                &env_with_xdg(empty.path()),
                &dead,
                &no_clock,
                &override_path,
            ),
        ),
        (
            "state_dir_unknown",
            resolve(&no_state_dir, &dead, &no_clock, &live_request()),
        ),
        (
            "service_file_missing",
            resolve(
                &env_with_xdg(empty.path()),
                &dead,
                &no_clock,
                &live_request(),
            ),
        ),
        (
            "service_file_unreadable",
            resolve(
                &env_with_xdg(unreadable.path()),
                &dead,
                &no_clock,
                &live_request(),
            ),
        ),
        (
            "service_file_malformed",
            resolve(
                &env_with_xdg(malformed.path()),
                &dead,
                &no_clock,
                &live_request(),
            ),
        ),
        (
            "host_pid_dead",
            resolve(
                &env_with_xdg(valid.path()),
                &dead,
                &no_clock,
                &live_request(),
            ),
        ),
        (
            "host_pid_recycled",
            resolve(
                &env_with_xdg(valid.path()),
                &alive,
                &recycled,
                &live_request(),
            ),
        ),
        (
            "host_url_unsupported",
            resolve(
                &env_with_xdg(unsupported.path()),
                &alive,
                &no_clock,
                &live_request(),
            ),
        ),
        (
            "host_url_not_loopback",
            resolve(
                &env_with_xdg(not_loopback.path()),
                &alive,
                &no_clock,
                &live_request(),
            ),
        ),
        (
            "host_model_unresolved",
            resolve(&env_with_xdg(valid.path()), &alive, &no_clock, &inherit),
        ),
    ];

    assert_eq!(cases.len(), HostWiringError::all().len());
    for (expected, outcome) in cases {
        let error = outcome.expect_err(expected);
        assert_eq!(
            error.reason(),
            expected,
            "the row's pinning input must reach it"
        );
    }

    // The closed set: ten variants, ten distinct strings, every one reached.
    let mut reasons: Vec<&'static str> = HostWiringError::all()
        .into_iter()
        .map(|error| error.reason())
        .collect();
    reasons.sort_unstable();
    reasons.dedup();
    assert_eq!(reasons.len(), 10, "no two variants share a reason string");
}

/// A DERIVED `Debug` would pass this and prove nothing (zeroize's own `Debug`
/// redacts); the mutant is a MANUAL formatter printing the inner value.
#[test]
fn test_wiring_and_request_debug_never_print_the_credential() {
    let dir = TestDir::new("host-endpoint-redaction");
    write_service(dir.path(), &service_json(49374, 4242, "s3cret", "2.2.5"));

    let wiring = resolve(
        &env_with_xdg(dir.path()),
        &FakeProbe::with(&[4242]),
        &FakeClock::empty(),
        &live_request(),
    )
    .expect("resolves");
    let rendered = format!("{wiring:?}");
    assert!(
        !rendered.contains("s3cret"),
        "the password leaks: {rendered}"
    );
    assert!(
        !rendered.contains("b3BlbmNvZGU6czNjcmV0"),
        "the base64 half leaks: {rendered}"
    );
    assert!(
        !rendered.contains("opencode:s3cret"),
        "the joined userinfo leaks: {rendered}"
    );

    let request = HostRequest {
        method: HostMethod::Post,
        url: "http://127.0.0.1:49374/api/session".to_string(),
        body: None,
        timeout: Duration::from_secs(1),
        cancellation: Default::default(),
        authorization: wiring.authorization.clone(),
    };
    let rendered = format!("{request:?}");
    assert!(
        !rendered.contains("s3cret"),
        "the password leaks: {rendered}"
    );
    assert!(
        !rendered.contains("b3BlbmNvZGU6czNjcmV0"),
        "the base64 half leaks: {rendered}"
    );
}
