//! The W6.3 dev harness: a real `EpisodicStore` behind the shipped router.
//!
//! What it proves end to end: the two session routes, the `EpisodicSessionPort`
//! adapter, the orphan flag, and the W6.5 session-history screen — served from
//! the same gate layer as production.
//!
//! ```text
//! cd web-ui && trunk build          # produces web-ui/dist
//! cargo run -p mem-web --example session_history_demo
//! ```
//!
//! **Demo-only token.** The session routes are token-gated even on loopback
//! (only `is_catalog_path` is D-11-public, `gate.rs`), so the harness supplies a
//! [`LocalTokenSource`] with the fixed value [`DEMO_TOKEN`]; the shell receives
//! it through the usual injected marker (`static_hosting.rs`) and the screen
//! sends it back. The token is a **demo constant, never a production
//! credential**: the server binds `127.0.0.1` only, and production reads the
//! real per-installation token through `FileLocalToken`.
//!
//! **No gate bypass.** `server::start` assembles the router with `build_router`,
//! so every request (static shell, API, fallback) goes through the D-8 Host
//! check, the Origin gate, the OPTIONS short-circuit, Basic auth and the token
//! step, in that order. The harness mounts **no** second `ServeDir` or fallback:
//! a second fallback would both bypass the gate and risk an axum merge panic.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mem_embed::embedder::{BoxFuture, Embedder};
use mem_embed::error::EmbedError;
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::EpisodicStore;
use mem_store::config::StoreConfig;
use mem_web::WebServerSettings;
use mem_web::api::ports::ApiServices;
use mem_web::api::store_adapter::episodic_session_port;
use mem_web::api_auth::LocalTokenSource;
use mem_web::host_allowlist::HostAllowlist;
use mem_web::server::{AppState, start};
use mem_web::web_auth::WebAuth;

/// The project the harness seeds (printed, and the screen's input default).
const PROJECT_ID: &str = "demo-project";
/// The session both seeded rows belong to (one session, two agents).
const SESSION_ID: &str = "demo-session";
/// The embedding dimension of the fixed-vector embedder and of the store.
const DIMENSIONS: u32 = 8;
/// The **demo-only** local token (see the module doc).
const DEMO_TOKEN: &str = "demo-token";

/// The fixed-vector embedder `insert_session` requires (no provider here).
struct DemoEmbedder;

impl Embedder for DemoEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, _text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        // A stable 8-dim unit-ish vector; every row embeds to the same point,
        // which is fine — the timeline read is chronological, not semantic.
        Box::pin(async { Ok(vec![0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0]) })
    }
}

/// The demo token source.
struct DemoTokenSource;

impl LocalTokenSource for DemoTokenSource {
    fn token(&self) -> Option<String> {
        Some(DEMO_TOKEN.to_string())
    }
}

/// The harness store config: a temp dir, encryption off, 8 dimensions.
fn demo_config(storage_path: PathBuf) -> StoreConfig {
    StoreConfig {
        storage_path,
        embedding_dimensions: DIMENSIONS,
        embedding_model: "demo-fixed".to_string(),
        max_vectors_per_shard: 100,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let storage =
        std::env::temp_dir().join(format!("opencode-mem-w63-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&storage);
    std::fs::create_dir_all(&storage)?;

    let store = Arc::new(EpisodicStore::new(
        storage.clone(),
        demo_config(storage.clone()),
    ));
    store.ready().await?;
    let embedder = DemoEmbedder;

    // Distinct `agent_id`s under ONE `session_id`: `UNIQUE (project_id,
    // session_id, agent_id)` forbids sharing the triple, and `insert_session`
    // pre-checks that the parent is live and in the same project.
    let parent = store
        .insert_session(
            NewAgentSession {
                project_id: PROJECT_ID.to_string(),
                session_id: SESSION_ID.to_string(),
                agent_id: "agent-parent".to_string(),
                agent_name: "Planner".to_string(),
                summary: "Parent session: scaffolded the explorer demo".to_string(),
                ..Default::default()
            },
            &embedder,
        )
        .await?;

    // A small gap so the newest-first ordering is unambiguous even without the
    // id tie-break.
    tokio::time::sleep(Duration::from_millis(5)).await;

    let child = store
        .insert_session(
            NewAgentSession {
                project_id: PROJECT_ID.to_string(),
                session_id: SESSION_ID.to_string(),
                agent_id: "agent-child".to_string(),
                agent_name: "Implementer".to_string(),
                parent_session_id: Some(parent.clone()),
                summary: "Child session: wired the session-history endpoints".to_string(),
                ..Default::default()
            },
            &embedder,
        )
        .await?;

    let dist_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web-ui/dist");

    let state = AppState {
        basic_auth: WebAuth::new(None, None, "demo"),
        // No configured API token: the local (demo) token is the only way in.
        api_token: None,
        local_token: Arc::new(DemoTokenSource),
        dist_dir: dist_dir.clone(),
        // The demo never serves real stats data; a throwaway dir keeps the
        // daily-stats scan hermetic.
        log_dir: std::env::temp_dir().join("opencode-mem-demo-logs"),
        // `start` re-derives this from the bound host; set for direct readers.
        host_allowlist: HostAllowlist::for_bind_host("127.0.0.1"),
        api: Arc::new(
            ApiServices::unavailable().with_sessions(episodic_session_port(Arc::clone(&store))),
        ),
        user_id: "demo@example.com".to_string(),
        bind_host: "127.0.0.1".to_string(),
    };

    let settings = WebServerSettings {
        enabled: true,
        // Loopback only: the demo never answers a network peer.
        host: "127.0.0.1".to_string(),
        // 0 = an OS-assigned free port (printed below).
        port: 0,
        auth_password: None,
        auth_username: None,
        api_token: None,
    };

    let handle = start(&settings, state)
        .await?
        .expect("the demo settings enable the server");
    let address = handle.local_addr();

    println!("OpenCode Memory Explorer — W6.3 session-history demo");
    println!("  UI:            http://{address}/sessions");
    println!("  project id:    {PROJECT_ID}");
    println!("  parent id:     {parent}");
    println!("  child id:      {child}");
    println!(
        "  delete the PARENT row in the UI to see the child's `orphaned` badge \
         (the parent id is printed above)."
    );
    println!("  demo token:    {DEMO_TOKEN}  (demo-only, loopback-only)");
    println!(
        "  API list:      curl -H 'x-opencode-mem-token: {DEMO_TOKEN}' \
         http://{address}/api/projects/{PROJECT_ID}/sessions"
    );
    println!(
        "  API delete:    curl -X DELETE -H 'x-opencode-mem-token: {DEMO_TOKEN}' \
         http://{address}/api/projects/{PROJECT_ID}/sessions/<session-row-id>"
    );
    println!("  static dist:   {}", dist_dir.display());
    if !dist_dir.join("index.html").is_file() {
        println!(
            "  NOTE: no {} yet — run `cd web-ui && trunk build` first.",
            dist_dir.join("index.html").display()
        );
    }
    println!("  episodic db:   {}", store.db_path().display());
    println!("Press Ctrl-C to stop.");

    // Keep alive without `tokio::signal` (the crate's `signal` feature is not
    // enabled); Ctrl-C terminates the process, closing the listener.
    std::future::pending::<()>().await;
    Ok(())
}
