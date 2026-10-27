//! Issue 52 — the single-source guards (D-52-6), and the stale-pointer
//! retirement (52.7).
//!
//! Tests 17 and 18 are **machine checks over two shapes**, not a general seal:
//! a `const`/`static` whose initialiser *begins* with a list literal and
//! contains one of the eight defaults (17), and the five exact predicate/type
//! names defined outside `injection.rs` (18). D-52-6 declares the five blind
//! spots (a `let` table, a `match` arm table, a differently-named predicate, a
//! whole new module with different names, and a constructor-built list such as
//! `LazyLock::new(|| vec![…])`); this file does not claim beyond them.
//!
//! Test 17's scanner is deliberately **line-wise**, not bracket-balancing:
//! the marker `"[SYSTEM DIRECTIVE: OH-MY-OPENCODE"` carries an unmatched `[`
//! inside a string literal, so a bracket-counting extractor truncates the item
//! and the guard goes vacuous. It reports the set, never a total — three scans
//! disagreed on the number of list-literal consts, and the set is the
//! assertion.
//!
//! The line-wise accumulator closes when the last non-whitespace character of
//! a line is a `;` **outside a trailing `//` comment** (`item_closes`), so a
//! multi-line string const (for example `capture.rs`'s
//! `RAW_CONTEXT_REQUIRED_MESSAGE`) closes itself and no longer swallows the
//! items that follow it, while `const L: [&str; 2] = [ // note;` and an
//! interior `// note;` line do not close it early. Six shapes were measured
//! against the predicate: an appended multi-line `LEAKED_MARKERS` at EOF
//! (missed by the old `];` rule, caught by the R4 `;` rule and by
//! `item_closes`); `… = [ // note;` (caught, missed, caught); an interior
//! `// note;` line (missed by R4, caught); `const MAX: u8 = 1; // note`
//! preceding the leak (missed by R4, caught); a closing `]; // note` (missed
//! by both earlier rules, caught); and a one-line `… ]; // note` (missed by
//! both, caught). A constructor-built list remains D-52-6's fifth blind spot
//! (its initialiser begins with an identifier, so the shape predicate excludes
//! it); the predicate itself is not widened. The sixth blind spot is the close
//! predicate's own, and two residual shapes remain: a `;` ending an
//! **unrecognised** comment line (an unterminated block comment `/* note;`,
//! or a `//` hidden behind a `"` in a char literal) and a `;` at the end of a
//! line inside a multi-line raw string. Both are declared, not caught.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MARKERS: [&str; 8] = [
    "<system-reminder>",
    "<!-- OMO_INTERNAL_INITIATOR -->",
    "<!-- OMO_INTERNAL_NOREPLY -->",
    "[SYSTEM DIRECTIVE: OH-MY-OPENCODE",
    "[Agent Usage Reminder]",
    "[Category+Skill Reminder]",
    "<team_mode_status",
    "<auto-slash-command>",
];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            if name == "target" || name == "dist" {
                continue;
            }
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

/// Every `.rs` file under `crates/*/src` and `src` (the scan roots).
fn scan_roots() -> Vec<PathBuf> {
    let root = manifest_dir();
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("crates")) {
        for entry in entries.flatten() {
            let src = entry.path().join("src");
            if src.is_dir() {
                collect_rust_files(&src, &mut files);
            }
        }
    }
    collect_rust_files(&root.join("src"), &mut files);
    files.sort();
    files
}

fn repo_relative(path: &Path) -> String {
    path.strip_prefix(manifest_dir())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `const`/`static` name when the line declares one (`pub`, `pub(crate)` ok).
fn declaration_name(line: &str) -> Option<String> {
    let mut rest = line.trim_start();
    if let Some(after) = rest.strip_prefix("pub") {
        rest = if let Some(after) = after.strip_prefix('(') {
            let close = after.find(')')?;
            after[close + 1..].trim_start()
        } else {
            after.trim_start()
        };
    }
    let rest = rest
        .strip_prefix("const ")
        .or_else(|| rest.strip_prefix("static "))?;
    let name: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    if name.is_empty() {
        return None;
    }
    // A `const`/`static` item has `name: Type` — `const fn` does not.
    if rest[name.len()..].trim_start().starts_with(':') {
        Some(name)
    } else {
        None
    }
}

/// True when `line` ends a `const`/`static` item: its last non-whitespace
/// character is a `;` outside a trailing `//` comment.
///
/// The comment rule is load-bearing in both directions: `const L: [&str; 2] =
/// [ // note;` and an interior `// note;` line end in a `;` that is not the
/// item terminator, while `const MAX: u8 = 1; // note` and a closing `]; //
/// note` do end the item even though the raw line does not end in `;`. A `//`
/// inside a double-quoted string (`"http://…"`) is not a comment.
fn item_closes(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            return line[..index].trim_end().ends_with(';');
        }
        index += 1;
    }
    line.trim_end().ends_with(';')
}

/// Marker-bearing list items in one file, as `(item name, matched markers)`.
fn marker_list_items(source: &str) -> Vec<(String, usize)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut items = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let Some(name) = declaration_name(lines[index]) else {
            index += 1;
            continue;
        };
        let text;
        if item_closes(lines[index]) {
            text = lines[index].to_string();
            index += 1;
        } else {
            let mut accumulated = vec![lines[index]];
            index += 1;
            let mut closed = false;
            while index < lines.len() {
                accumulated.push(lines[index]);
                index += 1;
                if accumulated.last().is_some_and(|line| item_closes(line)) {
                    closed = true;
                    break;
                }
            }
            // An accumulator that never closes is D-52-6's fifth blind spot
            // (constructor-built lists); the item is dropped, not guessed.
            if !closed {
                continue;
            }
            text = accumulated.join("\n");
        }
        let Some(equals) = text.find('=') else {
            continue;
        };
        let mut initialiser = text[equals + 1..].trim();
        if let Some(stripped) = initialiser.strip_suffix(';') {
            initialiser = stripped.trim();
        }
        if !initialiser.starts_with("&[")
            && !initialiser.starts_with('[')
            && !initialiser.starts_with("vec![")
            && !initialiser.starts_with("&vec![")
        {
            continue;
        }
        let matched = MARKERS
            .iter()
            .filter(|marker| initialiser.contains(*marker))
            .count();
        if matched > 0 {
            items.push((name, matched));
        }
    }
    items
}

// ---------------------------------------------------------------------------
// Step 16 — test 17: exactly one injection-marker list exists
// ---------------------------------------------------------------------------

#[test]
fn test_exactly_one_injection_marker_list_exists() {
    let mut found: BTreeSet<(String, String, usize)> = BTreeSet::new();
    for path in scan_roots() {
        let source = std::fs::read_to_string(&path).expect("read a source file");
        for (name, matched) in marker_list_items(&source) {
            found.insert((repo_relative(&path), name, matched));
        }
    }
    for (file, name, matched) in &found {
        println!("({file}, {name}, {matched})");
    }
    let expected: BTreeSet<(String, String, usize)> = [(
        "crates/mem-core/src/config/normalize.rs".to_string(),
        "DEFAULT_INJECTION_MARKERS".to_string(),
        8,
    )]
    .into_iter()
    .collect();
    assert_eq!(found, expected, "the marker-bearing list set is the gate");
}

// ---------------------------------------------------------------------------
// Step 16 — test 18: no second filter implementation
// ---------------------------------------------------------------------------

#[test]
fn test_no_second_injection_filter_implementation_exists() {
    let needles = [
        "fn contains_injection_marker",
        "fn is_injected_part",
        "fn filter_injected_parts",
        "fn is_internal_prompt",
        "struct InjectionFilter",
    ];
    let mut files: BTreeSet<String> = BTreeSet::new();
    for path in scan_roots() {
        let source = std::fs::read_to_string(&path).expect("read a source file");
        if needles.iter().any(|needle| source.contains(needle)) {
            files.insert(repo_relative(&path));
        }
    }
    let expected: BTreeSet<String> = ["crates/mem-ai/src/privacy/injection.rs".to_string()]
        .into_iter()
        .collect();
    assert_eq!(files, expected);
}

// ---------------------------------------------------------------------------
// Step 17 — test 19: the five stale deferral pointers are retired
// ---------------------------------------------------------------------------

fn assert_fragment_absent(file: &str, fragment: &str) {
    let source = std::fs::read_to_string(manifest_dir().join(file)).expect("read the file");
    let count = source
        .lines()
        .filter(|line| line.contains(fragment))
        .count();
    println!("{file}: {fragment:?} -> {count}");
    assert_eq!(count, 0, "{file} still carries {fragment:?}");
}

#[test]
fn test_the_five_stale_deferral_pointers_are_retired() {
    // Seven fragment assertions over five pointers, each on its own file and
    // its own fragment: `injection.rs:129` carries BOTH `plugin-host-config`
    // and `Phase 5` on one line, so one fragment alone would let a
    // half-retired pointer survive. The trailing period in `later wave.` is
    // deliberate — `normalize.rs:447`'s
    // `permission checks and key auto-creation are later waves' scope.` is an
    // UNRELATED doc on `resolve_secret_value` and must not be deleted to make
    // a loose pattern pass.
    assert_fragment_absent("crates/mem-ai/src/privacy/mod.rs", "plugin-host-config");
    assert_fragment_absent("crates/mem-ai/src/privacy/mod.rs", "Phase 5");
    assert_fragment_absent(
        "crates/mem-ai/src/privacy/injection.rs",
        "plugin-host-config",
    );
    assert_fragment_absent("crates/mem-ai/src/privacy/injection.rs", "Phase 5");
    assert_fragment_absent("crates/mem-ai/CONTEXT.md", "Phase 5");
    assert_fragment_absent(
        "crates/mem-core/src/config/normalize.rs",
        "filter functions themselves",
    );
    assert_fragment_absent("crates/mem-core/src/config/normalize.rs", "later wave.");
}
