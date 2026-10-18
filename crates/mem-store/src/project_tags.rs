//! Project-tag seam (blueprint W1.2 step 11) — the inventory/migration
//! layers get project identity through one injectable source so tests stay
//! hermetic (no `git` spawns).
//!
//! `CoreProjectTagSource` wires [`mem_core::tags::TagResolver`] with the
//! production env/git probe per call (upstream `getProjectTagInfo` with
//! `CONFIG` injected).

#![allow(dead_code)]

use std::path::Path;

use mem_core::env::ProcessEnv;
use mem_core::tags::probe::SystemGitProbe;
use mem_core::tags::{TagInfo, TagResolver};

/// Infallible like upstream (`getProjectTagInfo` never throws).
pub trait ProjectTagSource: Send + Sync {
    fn project_tag_info(&self, directory: &Path) -> TagInfo;
}

/// Production source over mem-core's resolver + real git probe.
pub struct CoreProjectTagSource {
    pub container_tag_prefix: String,
    pub user_email_override: Option<String>,
    pub user_name_override: Option<String>,
}

impl ProjectTagSource for CoreProjectTagSource {
    fn project_tag_info(&self, directory: &Path) -> TagInfo {
        // `static ENV` supplies the `'static` borrow expected by the probe.
        static TAGS_ENV: ProcessEnv = ProcessEnv;
        let git = SystemGitProbe::new(&TAGS_ENV);
        let resolver = TagResolver::new(
            &self.container_tag_prefix,
            self.user_email_override.as_deref(),
            self.user_name_override.as_deref(),
            &TAGS_ENV,
            &git,
        );
        resolver.project_tag_info(directory)
    }
}
