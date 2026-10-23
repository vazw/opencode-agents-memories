//! Session-end vault feeder (W8a).
//!
//! # What this module is
//!
//! The producer that promotes *user-marked* verbatim content into the vault
//! tier. It runs as a spawned async task off the capture write path and never
//! blocks capture, never panics, and never writes to episodic/facts.
//!
//! # User-marked only (v1 predicate)
//!
//! A candidate is extracted only from:
//!   - `<vault>...</vault>` regions (inner text),
//!   - `<verbatim>...</verbatim>` regions (inner text),
//!   - the whole retained raw context when `CaptureParams.vault == true`.
//! Unmarked content is never promoted.
//!
//! # Private-content stripping is upstream (capture), NOT here
//!
//! Capture already strips `<private>` regions before retention
//! (`mem_ai::privacy::strip_private_content` at `capture.rs`), drops fully
//! private payloads to `raw_context: None`, and retains the stripped text.
//! The feeder receives the retained (already private-stripped) raw context and
//! stores it verbatim. The feeder performs NO tag-privacy check: if it is
//! ever handed an unstripped string, that is a caller bug, not a feeder
//! responsibility.
//!
//! # No feeder size cap
//!
//! The input is already bounded upstream by `MAX_RAW_CONTEXT_BYTES`
//! (256 KiB), enforced at capture and re-validated in
//! `EpisodicStore::insert_raw_context`. A candidate is never truncated:
//! truncation destroys verbatimness. (If a cap is ever reintroduced it must
//! skip, not truncate.)
//!
//! # No secret detection (honest exposure)
//!
//! v1 performs NO secret detection. `mem-ai` provides no free-text secret
//! detector: `validate_secret_shape` validates a value already asserted to be
//! a secret (answers "is this a plausible secret?", never "does this text
//! contain one?"); its `Ok` is true of most single-line config. Using it as
//! a gate is a category error in either direction (a normal marked paragraph
//! has whitespace -> promoted; a single-line candidate with no spaces ->
//! dropped whole, i.e. exactly the "exact configs" class the tier exists to
//! hold; an `api_key = "sk-live-..."` inside a paragraph -> promoted, secret
//! stored). The feeder therefore stores what a human marked. A marked
//! candidate that contains a resolved secret is stored, exactly as marked.
//! The drop-whole policy is deferred with the machine-checkable detectors.
//!
//! # References stored, never resolved
//!
//! The feeder never calls a resolver. An `env://`/`file://` reference is
//! stored as the literal string and is never turned into a value.
//!
//! # Verbatim promise (precision)
//!
//! The tier stores the candidate byte-identical to the bytes the feeder
//! received — i.e. the retained raw context, which capture has already
//! private-stripped. It is NOT byte-identical to the original session
//! transcript. Round-trip equality is asserted against the feeder's actual
//! input.
//!
//! # Deferred machine-checkable detectors
//!
//! Code fences, config patterns, secret-reference lines, embedded-secret
//! tokens in prose, and whole-candidate secret-shape detection are deferred
//! until the vault has an erasure path. Trigger: landing of the purge erasure
//! leg (`PurgeRequest.vault_scopes` / `VaultLeg` / `VaultPurgeReport`) in
//! W8b — until then a false-positive promotion into an immutable tier cannot
//! be undone. Additional reason: `mem-ai` has no free-text secret detector
//! (`validate_secret_shape` validates a value already asserted to be a
//! secret; it never scans prose).
//!
//! # Error handling
//!
//! `EncryptionRequired` sets a process-wide disabled flag and no-ops
//! thereafter (logged once). Other errors are logged with scope, retried up to
//! 3 times with backoff, then dropped and counted. The feeder never blocks
//! capture.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use mem_vault::{VaultEntry, VaultStore};

use crate::capture::CaptureParams;
use crate::project_scope::ResolvedProjectScope;
use crate::vault_source_ref::{VaultSourceKind, vault_source_ref};

pub struct VaultFeederJob {
    pub scope: ResolvedProjectScope,
    pub raw_context: String,
    pub params: CaptureParams,
}

pub struct VaultFeeder {
    vault: Arc<VaultStore>,
    disabled: Arc<AtomicBool>,
    drops: Arc<AtomicU64>,
    enqueued: Arc<AtomicU64>,
}

impl VaultFeeder {
    pub fn new(vault: Arc<VaultStore>) -> Self {
        Self {
            vault,
            disabled: Arc::new(AtomicBool::new(false)),
            drops: Arc::new(AtomicU64::new(0)),
            enqueued: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::SeqCst)
    }

    pub fn drop_count(&self) -> u64 {
        self.drops.load(Ordering::SeqCst)
    }

    pub fn enqueue_count(&self) -> u64 {
        self.enqueued.load(Ordering::SeqCst)
    }

    pub async fn run(&self, job: VaultFeederJob) {
        Self::run_inner(
            self.vault.clone(),
            self.disabled.clone(),
            self.drops.clone(),
            job,
        )
        .await;
    }

    pub fn enqueue(&self, job: VaultFeederJob) {
        self.enqueued.fetch_add(1, Ordering::SeqCst);
        let vault = self.vault.clone();
        let disabled = self.disabled.clone();
        let drops = self.drops.clone();
        tokio::spawn(async move {
            Self::run_inner(vault, disabled, drops, job).await;
        });
    }

    async fn run_inner(
        vault: Arc<VaultStore>,
        disabled: Arc<AtomicBool>,
        drops: Arc<AtomicU64>,
        job: VaultFeederJob,
    ) {
        if disabled.load(Ordering::SeqCst) {
            return;
        }

        let scope = job.scope.scope_hash.clone();
        let mut candidates: Vec<String> = Vec::new();

        for tag in ["vault", "verbatim"] {
            for inner in extract_tagged(&job.raw_context, tag) {
                candidates.push(inner);
            }
        }
        if job.params.vault {
            candidates.push(job.raw_context.clone());
        }

        let source_ref = vault_source_ref(
            VaultSourceKind::Session,
            &format!(
                "{}:{}:{}",
                job.params.project_id, job.params.session_id, job.params.agent_id
            ),
        );
        let created_by = if job.params.agent_id.trim().is_empty() {
            "vault-feeder".to_string()
        } else {
            job.params.agent_id.clone()
        };

        for candidate in candidates {
            if disabled.load(Ordering::SeqCst) {
                break;
            }
            let entry = VaultEntry {
                content: candidate.into_bytes(),
                mime_type: None,
                source_ref: source_ref.clone(),
                created_by: created_by.clone(),
            };

            let mut attempt = 0u32;
            loop {
                attempt += 1;
                match vault.put(&scope, entry.clone()).await {
                    Ok(_) => break,
                    Err(mem_vault::VaultError::EncryptionRequired) => {
                        disabled.store(true, Ordering::SeqCst);
                        tracing::warn!("vault feeder disabled: encryption required");
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(scope = %scope, error = %error, "vault feeder put failed");
                        if attempt >= 3 {
                            drops.fetch_add(1, Ordering::SeqCst);
                            break;
                        }
                        let backoff = Duration::from_millis(10 * attempt as u64);
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }
    }
}

fn extract_tagged(text: &str, tag: &str) -> Vec<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        match after.find(&close) {
            Some(end) => {
                out.push(after[..end].to_string());
                rest = &after[end + close.len()..];
            }
            None => break,
        }
    }
    out
}
