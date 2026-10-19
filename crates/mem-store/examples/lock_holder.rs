//! Cross-process lock holder — helper binary for the W1.5 cross-process
//! lock tests (blueprint step 2, captain-built; consumed by lane sL).
//!
//! Usage: `lock_holder --storage <dir> --hold-ms <n> [--wait-ms <n>]`
//!
//! Acquires the `.turso-operation.lock` with operation `"hold"`, sleeps
//! `--hold-ms`, releases, exits 0. When acquisition fails it prints
//! `LOCKED` to stderr and exits 2. `--wait-ms` (default 0) is an OPTIONAL
//! bounded retry budget: the helper re-attempts acquisition every 20 ms
//! until the budget elapses, so two overlapping holders can *serialize*
//! (both exit 0) — without it the second holder is refused immediately
//! (the pinned live-holder contract).
//!
//! stdout/stderr are meant to be redirected to files by the spawning test
//! (never pipes) per `docs/agents/testing.md`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let mut storage: Option<PathBuf> = None;
    let mut hold_ms: u64 = 0;
    let mut wait_ms: u64 = 0;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--storage" => storage = args.next().map(PathBuf::from),
            "--hold-ms" => hold_ms = parse_ms(args.next()),
            "--wait-ms" => wait_ms = parse_ms(args.next()),
            _ => {}
        }
    }

    let Some(storage) = storage else {
        eprintln!("lock_holder: missing --storage");
        std::process::exit(64);
    };
    if let Err(error) = std::fs::create_dir_all(&storage) {
        eprintln!("lock_holder: cannot create storage dir: {error}");
        std::process::exit(64);
    }

    let probe: Arc<dyn mem_store::platform::ProcessProbe> =
        Arc::new(mem_store::platform::SystemProcessProbe);

    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    let lock = loop {
        match mem_store::operation_lock::acquire_turso_operation_lock(
            &storage,
            probe.clone(),
            "hold",
        ) {
            Ok(lock) => break lock,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                eprintln!("LOCKED");
                std::process::exit(2);
            }
        }
    };

    std::thread::sleep(Duration::from_millis(hold_ms));
    lock.release();
    std::process::exit(0);
}

fn parse_ms(value: Option<String>) -> u64 {
    value.and_then(|text| text.parse().ok()).unwrap_or(0)
}
