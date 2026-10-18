//! Windows-path handling contract: separator normalization and project-name
//! extraction ported from upstream `src/services/tags.ts` `getProjectName`
//! (the subject of `tests/windows-path.test.ts`).
//!
//! `normalize_separators` is the shared cross-platform idiom upstream
//! applies inline in `tags.ts`, `shard-inventory-service.ts`,
//! `shard-manager.ts`, and `user-prompt-manager.ts` (later consumed by
//! W1.1/W1.2/W3.1/W6.x): it returns the input unchanged when there are no
//! backslashes.
//!
//! Documented divergence: host-dependent Node root semantics for
//! pathological foreign root paths (`C:\..`, `/..`) are not emulated — the
//! `..` stack collapse here is the lexical part of upstream
//! `path.normalize` that survives into the result. Inputs are
//! display/identity strings, not filesystem operations; no drive-letter or
//! UNC special cases are added.

/// The shared upstream idiom: convert all backslashes to forward slashes.
pub fn normalize_separators(path: &str) -> String {
    path.replace('\\', "/")
}

/// Extracts the project name from a directory path: the last segment after
/// separator normalization and lexical `.`/`..` collapse.
///
/// Falls back to the RAW input string when no segment survives (upstream
/// `parts[parts.length-1] || directory`) — never returns an empty string
/// for `""`, `"/"`, or `"."`.
pub fn get_project_name(directory: &str) -> String {
    let normalized = normalize_separators(directory);
    let mut segments: Vec<&str> = Vec::new();
    for seg in normalized.split('/') {
        match seg {
            "" | "." => {}
            ".." => match segments.last() {
                Some(&last) if last != ".." => {
                    segments.pop();
                }
                _ => segments.push(".."),
            },
            other => segments.push(other),
        }
    }
    segments
        .last()
        .map(|s| (*s).to_string())
        .unwrap_or_else(|| directory.to_string())
}
