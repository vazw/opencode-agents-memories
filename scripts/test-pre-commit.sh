#!/usr/bin/env bash
# Behavioural self-test for the tracked pre-commit gate.
#
# Every case drives a real `git commit` inside a throwaway repository under
# <repo>/.tmp/scratch/precommit-selftest/ that carries `core.hooksPath .githooks`
# and a copy of the tracked hook. The harness never writes to this repository's
# index or worktree; its only git calls against the real repo are read-only
# (`rev-parse`, `ls-files`).
#
# Usage: bash scripts/test-pre-commit.sh
# Exit:  0 when all 16 cases pass, 1 otherwise.
set -uo pipefail

SECONDS=0

repo_root="$(git rev-parse --show-toplevel)" || {
    printf 'test-pre-commit: cannot resolve the repository root\n' >&2
    exit 1
}
hook="$repo_root/.githooks/pre-commit"
scratch_root="$repo_root/.tmp/scratch"
work="$scratch_root/precommit-selftest"

# --- work-dir guard: never the repository root, never outside <repo>/.tmp/scratch ---
mkdir -p "$work" || {
    printf 'test-pre-commit: cannot create the work dir %s\n' "$work" >&2
    exit 1
}
work_real="$(cd "$work" && pwd -P)"
scratch_real="$(cd "$scratch_root" && pwd -P)"
repo_real="$(cd "$repo_root" && pwd -P)"
case "$work_real" in
    "$scratch_real"/*) ;;
    *)
        printf 'test-pre-commit: refusing to run: %s is not under %s\n' "$work_real" "$scratch_real" >&2
        exit 1
        ;;
esac
if [ "$work_real" = "$repo_real" ]; then
    printf 'test-pre-commit: refusing to run: the work dir resolves to the repository root\n' >&2
    exit 1
fi

real_cargo="$(command -v cargo || true)"

# Fresh fixtures every run; everything stays under .tmp/scratch.
rm -rf "$work"/repo.*
harness_tmp="$work/tmp"
rm -rf "$harness_tmp"
mkdir -p "$harness_tmp"

repo=""
commit_out=""
commit_rc=0
case_reason=""

fail() {
    case_reason="$1"
    return 1
}

# --- helpers ---------------------------------------------------------------

mk_repo() {
    if [ ! -x "$hook" ]; then
        fail "hook not found: $hook is missing or not executable"
        return 1
    fi
    repo="$(mktemp -d "$work/repo.XXXXXX")" || {
        fail "cannot create a fixture repository under $work"
        return 1
    }
    git -C "$repo" init -q || {
        fail "git init failed in the fixture"
        return 1
    }
    git -C "$repo" config user.name "pre-commit selftest"
    git -C "$repo" config user.email "selftest@example.invalid"
    printf '/.tmp/\n/target/\n/web-ui/target/\n' > "$repo/.gitignore"
    mkdir -p "$repo/.githooks" "$repo/.tmp"
    cp -p "$hook" "$repo/.githooks/pre-commit" || {
        fail "cannot copy the hook into the fixture"
        return 1
    }
    git -C "$repo" config core.hooksPath .githooks
    return 0
}

# The pinned fixture (D10): rustfmt-clean and clippy-clean under both gates.
# The initial commit bypasses the hook on purpose: it is scaffolding, and
# leaving .tmp/scratch absent in the fixture is what makes the later cases
# prove the hook bootstraps its own scratch root.
mk_workspace() {
    local repo_dir="$1"
    mkdir -p "$repo_dir/crates/probe/src"
    cat > "$repo_dir/Cargo.toml" <<'MANIFEST'
[workspace]
members = ["crates/*"]
exclude = ["web-ui"]
resolver = "3"
MANIFEST
    cat > "$repo_dir/crates/probe/Cargo.toml" <<'MANIFEST'
[package]
name = "probe"
version = "0.0.0"
edition = "2024"
MANIFEST
    cat > "$repo_dir/crates/probe/src/lib.rs" <<'RUST'
//! Probe fixture for the pre-commit harness.

/// Adds two lane values.
pub fn add_lanes(a: u8, b: u8) -> u8 {
    a + b
}
RUST
    cat > "$repo_dir/crates/probe/src/extra.rs" <<'RUST'
//! Extra source used only by the deletion case.
RUST
    (
        cd "$repo_dir" &&
            TMPDIR="$harness_tmp" CARGO_TARGET_DIR="$repo_dir/target" cargo generate-lockfile --quiet
    ) || {
        fail "cargo generate-lockfile failed in the fixture"
        return 1
    }
    git -C "$repo_dir" add -A >/dev/null
    git -C "$repo_dir" commit -q --no-verify -m "fixture" || {
        fail "the fixture's initial commit failed"
        return 1
    }
    return 0
}

run_commit() {
    local repo_dir="$1" msg="$2" dir="${3:-$1}"
    commit_out="$(cd "$dir" && git commit -m "$msg" 2>&1)"
    commit_rc=$?
    return 0
}

assert_contains() {
    case "$commit_out" in
        *"$1"*) return 0 ;;
        *)
            fail "the hook output did not contain: $1"
            return 1
            ;;
    esac
}

assert_not_contains() {
    case "$commit_out" in
        *"$1"*)
            fail "the hook output unexpectedly contained: $1"
            return 1
            ;;
        *) return 0 ;;
    esac
}

write_unformatted_lib() {
    cat > "$1/crates/probe/src/lib.rs" <<'RUST'
//! Probe fixture for the pre-commit harness.

/// Adds two lane values.
pub fn add_lanes(a:u8,b:u8)->u8{
	a+b
}
RUST
}

# Every PATH entry except the rustup proxy dir; the hook must fail loudly rather
# than fall back to an inherited toolchain.
path_without_cargo() {
    local -a parts=() kept=()
    IFS=':' read -r -a parts <<< "$PATH"
    local part
    for part in "${parts[@]}"; do
        case "$part" in
            */.cargo | */.cargo/ | */.cargo/*) continue ;;
        esac
        kept+=("$part")
    done
    local stripped="" piece
    for piece in "${kept[@]}"; do
        stripped="${stripped:+$stripped:}$piece"
    done
    printf '%s\n' "$stripped"
}

# --- cases -----------------------------------------------------------------

case_01_hook_is_executable() {
    [ -x "$hook" ] || {
        fail "hook not found: $hook is missing or not executable"
        return 1
    }
    local mode
    mode="$(git -C "$repo_root" ls-files -s -- .githooks/pre-commit | awk '{ print $1 }')"
    if [ -z "$mode" ]; then
        printf 'SKIP index mode (untracked): the orchestrator stages the hook after this run\n'
        return 0
    fi
    [ "$mode" = "100755" ] || {
        fail "the index mode is $mode, expected 100755"
        return 1
    }
    return 0
}

case_02_docs_only_commit_skips_cargo() {
    mk_repo || return 1
    local shim_dir="$work/shim-docs-$$" marker="$work/cargo-marker-$$"
    rm -f "$marker"
    mkdir -p "$shim_dir"
    cat > "$shim_dir/cargo" <<SHIM
#!/usr/bin/env bash
printf 'cargo invoked\n' > "$marker"
exit 1
SHIM
    chmod 755 "$shim_dir/cargo"
    printf '# notes\n' > "$repo/notes.md"
    git -C "$repo" add notes.md
    local saved_path="$PATH"
    PATH="$shim_dir:$PATH"
    run_commit "$repo" "case 02 docs only"
    PATH="$saved_path"
    [ "$commit_rc" -eq 0 ] || {
        fail "the docs-only commit was refused (rc $commit_rc)"
        return 1
    }
    [ ! -e "$marker" ] || {
        fail "cargo was invoked for a docs-only commit"
        return 1
    }
    return 0
}

case_03_unformatted_staged_rs_blocks() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    local head_before
    head_before="$(git -C "$repo" rev-parse HEAD)"
    write_unformatted_lib "$repo"
    git -C "$repo" add crates/probe/src/lib.rs
    run_commit "$repo" "case 03 unformatted"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit was not refused (rc 0) — no fmt phase"
        return 1
    }
    assert_contains "FAIL fmt" || return 1
    assert_contains "crates/probe/src/lib.rs" || return 1
    # The awk rewrite must turn the temp blob path in the diff header into the
    # repo-relative path (the diagnostic a reader actually needs).
    assert_contains "Diff in crates/probe/src/lib.rs" || return 1
    [ "$(git -C "$repo" rev-parse HEAD)" = "$head_before" ] || {
        fail "HEAD advanced although the commit was refused"
        return 1
    }
    return 0
}

case_04_clean_staged_rs_passes() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    cat >> "$repo/crates/probe/src/lib.rs" <<'RUST'

/// Subtracts two lane values.
pub fn sub_lanes(a: u8, b: u8) -> u8 {
    a - b
}
RUST
    git -C "$repo" add crates/probe/src/lib.rs
    local msg="case 04 clean fixture"
    run_commit "$repo" "$msg"
    [ "$commit_rc" -eq 0 ] || {
        fail "expected the commit to succeed, got rc $commit_rc"
        return 1
    }
    [ "$(git -C "$repo" log -1 --format=%s)" = "$msg" ] || {
        fail "the last commit message is not the case message"
        return 1
    }
    return 0
}

case_05_unwrap_in_lib_blocks_via_panic_gate() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    cat > "$repo/crates/probe/src/lib.rs" <<'RUST'
//! Probe fixture for the pre-commit harness.

/// Reads the first lane, or panics.
pub fn first_lane(v: Option<u8>) -> u8 {
    v.unwrap()
}
RUST
    git -C "$repo" add crates/probe/src/lib.rs
    run_commit "$repo" "case 05 unwrap"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit was accepted; gate 1 never ran"
        return 1
    }
    assert_contains "FAIL clippy [gate 1/2" || return 1
    return 0
}

case_06_eight_argument_fn_blocks_via_all_targets_gate() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    cat > "$repo/crates/probe/src/lib.rs" <<'RUST'
//! Probe fixture for the pre-commit harness.

/// Adds eight lane values.
pub fn eight_lanes(a: u8, b: u8, c: u8, d: u8, e: u8, f: u8, g: u8, h: u8) -> u8 {
    a + b + c + d + e + f + g + h
}
RUST
    git -C "$repo" add crates/probe/src/lib.rs
    run_commit "$repo" "case 06 eight arguments"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit was accepted; gate 2 never ran"
        return 1
    }
    assert_contains "FAIL clippy [gate 2/2" || return 1
    assert_contains "too many arguments" || return 1
    return 0
}

case_07_fmt_judges_the_index_not_the_worktree() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    write_unformatted_lib "$repo"
    git -C "$repo" add crates/probe/src/lib.rs
    # Fix the worktree copy only; the index still holds the unformatted blob.
    cat > "$repo/crates/probe/src/lib.rs" <<'RUST'
//! Probe fixture for the pre-commit harness.

/// Adds two lane values.
pub fn add_lanes(a: u8, b: u8) -> u8 {
    a + b
}
RUST
    run_commit "$repo" "case 07 index vs worktree"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit was not refused (rc 0) — no fmt phase"
        return 1
    }
    assert_contains "FAIL fmt" || return 1
    assert_contains "crates/probe/src/lib.rs" || return 1
    return 0
}

case_08_worktree_edit_warns_but_does_not_block() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    cat >> "$repo/crates/probe/src/lib.rs" <<'RUST'

/// Doubles one lane value.
pub fn double_lane(a: u8) -> u8 {
    2 * a
}
RUST
    git -C "$repo" add crates/probe/src/lib.rs
    # A formatting-only worktree edit that is never staged.
    printf '\n// worktree-only comment\n' >> "$repo/crates/probe/src/lib.rs"
    run_commit "$repo" "case 08 worktree warn"
    [ "$commit_rc" -eq 0 ] || {
        fail "the worktree-only edit blocked the commit (rc $commit_rc)"
        return 1
    }
    if ! assert_contains "WARN"; then
        case_reason="the commit succeeded but no WARN line was printed — no fmt phase"
        return 1
    fi
    return 0
}

case_09_deleted_staged_rs_is_not_checked() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    cat >> "$repo/crates/probe/src/lib.rs" <<'RUST'

/// Doubles one lane value.
pub fn double_lane(a: u8) -> u8 {
    2 * a
}
RUST
    git -C "$repo" add crates/probe/src/lib.rs
    git -C "$repo" rm -q crates/probe/src/extra.rs
    run_commit "$repo" "case 09 deleted staged rs"
    [ "$commit_rc" -eq 0 ] || {
        fail "the commit was refused (rc $commit_rc); the D entry was not filtered"
        return 1
    }
    assert_not_contains "cannot read the staged blob" || return 1
    [ "$(git -C "$repo" log -1 --format=%s)" = "case 09 deleted staged rs" ] || {
        fail "the commit did not land"
        return 1
    }
    return 0
}

case_10_path_with_spaces_is_checked() {
    mk_repo || return 1
    mkdir -p "$repo/dir with space"
    cat > "$repo/dir with space/a.rs" <<'RUST'
//! Spaced-path fixture.
pub fn spaced(a:u8)->u8{a}
RUST
    git -C "$repo" add "dir with space/a.rs"
    run_commit "$repo" "case 10 path with spaces"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit was not refused (rc 0) — no fmt phase"
        return 1
    }
    assert_contains "FAIL fmt" || return 1
    assert_contains "dir with space/a.rs" || return 1
    return 0
}
case_11_missing_cargo_fails_loudly() {
    mk_repo || return 1
    printf 'placeholder\n' > "$repo/notes.md"
    git -C "$repo" add notes.md
    local saved_path="$PATH"
    PATH="$(path_without_cargo)"
    run_commit "$repo" "case 11 missing cargo"
    PATH="$saved_path"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit succeeded although cargo is absent"
        return 1
    }
    assert_contains "FAIL environment" || return 1
    assert_contains "cargo not found on PATH" || return 1
    return 0
}

case_12_missing_rustfmt_fails_loudly() {
    mk_repo || return 1
    # The system has /usr/bin/rustfmt as well as the rustup proxy, so stripping
    # `/.cargo/` alone would leave rustfmt resolvable. This fixture PATH carries
    # every tool the commit path needs (bash, sh, git, a cargo shim) and no
    # rustfmt at all.
    local bindir="$work/bin-case12-$$"
    mkdir -p "$bindir"
    local tool real
    for tool in bash sh git; do
        real="$(command -v "$tool")" || {
            fail "the harness cannot find $tool on PATH"
            return 1
        }
        ln -s "$real" "$bindir/$tool"
    done
    cat > "$bindir/cargo" <<SHIM
#!/usr/bin/env bash
exec "$real_cargo" "\$@"
SHIM
    chmod 755 "$bindir/cargo"
    if PATH="$bindir" command -v rustfmt > /dev/null 2>&1; then
        fail "the fixture PATH still resolves rustfmt; the assertion would be vacuous"
        return 1
    fi
    printf 'placeholder\n' > "$repo/notes.md"
    git -C "$repo" add notes.md
    local saved_path="$PATH"
    PATH="$bindir"
    run_commit "$repo" "case 12 missing rustfmt"
    PATH="$saved_path"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit succeeded although rustfmt is absent"
        return 1
    }
    assert_contains "FAIL environment" || return 1
    assert_contains "rustfmt not found on PATH" || return 1
    return 0
}
case_13_commit_from_subdirectory_is_gated() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    mkdir -p "$repo/sub"
    write_unformatted_lib "$repo"
    git -C "$repo" add crates/probe/src/lib.rs
    run_commit "$repo" "case 13 from subdir" "$repo/sub"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit was not refused (rc 0) — no fmt phase"
        return 1
    }
    assert_contains "FAIL fmt" || return 1
    return 0
}
case_14_hook_does_not_modify_the_tree() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    write_unformatted_lib "$repo"
    git -C "$repo" add crates/probe/src/lib.rs
    local before after
    before="$(git -C "$repo" status --porcelain)"
    run_commit "$repo" "case 14 no tree mutation"
    after="$(git -C "$repo" status --porcelain)"
    [ "$commit_rc" -ne 0 ] || {
        fail "expected the fmt phase to refuse the commit, got rc 0"
        return 1
    }
    assert_contains "FAIL fmt" || return 1
    [ "$before" = "$after" ] || {
        fail "the hook changed the worktree status"
        return 1
    }
    [ -n "$(git -C "$repo" ls-files -- Cargo.lock)" ] || {
        fail "the fixture's Cargo.lock is not committed"
        return 1
    }
    return 0
}
case_15_webui_change_runs_both_webui_gates() {
    mk_repo || return 1
    mk_workspace "$repo" || return 1
    mkdir -p "$repo/web-ui/src"
    cat > "$repo/web-ui/Cargo.toml" <<'MANIFEST'
[workspace]
[package]
name = "web-ui"
version = "0.0.0"
edition = "2024"
MANIFEST

    # Commit 1: a panic-family site. gate 1/2 must refuse the commit.
    cat > "$repo/web-ui/src/lib.rs" <<'RUST'
//! Web UI fixture.

/// Returns the first lane.
pub fn first_lane(v: Option<u8>) -> u8 {
    v.unwrap()
}
RUST
    git -C "$repo" add web-ui/Cargo.toml web-ui/src/lib.rs
    run_commit "$repo" "case 15 web-ui panic gate"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit was accepted; the web-ui panic gate never ran"
        return 1
    }
    assert_contains "FAIL clippy [gate 1/2: panic families — web-ui]" || return 1

    # Commit 2: panic-free but all-targets-dirty. gate 1/2 must pass and 2/2 must refuse.
    cat > "$repo/web-ui/src/lib.rs" <<'RUST'
//! Web UI fixture.

/// Adds eight lane values.
pub fn add_eight(a: u8, b: u8, c: u8, d: u8, e: u8, f: u8, g: u8, h: u8) -> u8 {
    a + b + c + d + e + f + g + h
}
RUST
    git -C "$repo" add web-ui/src/lib.rs
    run_commit "$repo" "case 15 web-ui all-targets gate"
    [ "$commit_rc" -ne 0 ] || {
        fail "the commit was accepted; the web-ui all-targets gate never ran"
        return 1
    }
    assert_contains "FAIL clippy [gate 2/2: all-targets warnings — web-ui]" || return 1
    assert_contains "too many arguments" || return 1
    return 0
}
case_16_docs_name_the_installed_configuration() {
    local agents="$repo_root/AGENTS.md"
    local adr="$repo_root/docs/adr/0016-pre-commit-gate.md"
    if [ ! -f "$agents" ] || [ ! -f "$adr" ]; then
        fail "docs absent: AGENTS.md or docs/adr/0016-pre-commit-gate.md is missing"
        return 1
    fi
    grep -F -q 'git config core.hooksPath .githooks' "$agents" || {
        fail "docs absent: AGENTS.md does not name the install command"
        return 1
    }
    grep -F -q 'scripts/test-pre-commit.sh' "$agents" || {
        fail "docs absent: AGENTS.md does not name the harness"
        return 1
    }
    grep -F -q 'cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used' "$adr" || {
        fail "docs absent: ADR-0016 does not carry gate 1 verbatim"
        return 1
    }
    grep -F -q 'cargo clippy --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation' "$adr" || {
        fail "docs absent: ADR-0016 does not carry gate 2 verbatim"
        return 1
    }
    # The web-ui phase runs both gates, panic families first (ADR-0016 D-3, second
    # amendment). Pin them in all four forms: each bare string the adr must carry,
    # each invocation with its directory (a bare string alone can be a prefix of the
    # root gate-2 line above, so it cannot pin the web-ui phase by itself), and the
    # two failure labels the hook actually prints.
    grep -F -q 'cargo clippy --workspace --all-targets -- -D warnings' "$adr" || {
        fail "docs absent: ADR-0016 does not carry the web-ui gate verbatim"
        return 1
    }
    grep -F -q 'cd web-ui && cargo clippy --workspace --all-targets -- -D warnings' "$adr" || {
        fail "docs absent: ADR-0016 does not attribute the all-targets command to web-ui"
        return 1
    }
    grep -F -q 'cd web-ui && cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used' "$adr" || {
        fail "docs absent: ADR-0016 does not attribute the panic-family command to web-ui"
        return 1
    }
    grep -F -q 'gate 1/2: panic families — web-ui' "$adr" || {
        fail "docs absent: ADR-0016 does not name the web-ui panic-family gate label"
        return 1
    }
    grep -F -q 'gate 2/2: all-targets warnings — web-ui' "$adr" || {
        fail "docs absent: ADR-0016 does not name the web-ui all-targets gate label"
        return 1
    }
    return 0
}

# --- runner ----------------------------------------------------------------

ok=0

run() {
    local fn="$1"
    case_reason=""
    repo=""
    commit_out=""
    commit_rc=0
    if "$fn"; then
        ok=$((ok + 1))
        printf 'ok   %s\n' "$fn"
    else
        printf 'FAIL %s — %s\n' "$fn" "${case_reason:-unknown failure}"
        if [ -n "$commit_out" ]; then
            printf '%s\n' "$commit_out" | sed -n '1,20p' | sed 's/^/       /'
        fi
    fi
}

run case_01_hook_is_executable
run case_02_docs_only_commit_skips_cargo
run case_03_unformatted_staged_rs_blocks
run case_04_clean_staged_rs_passes
run case_05_unwrap_in_lib_blocks_via_panic_gate
run case_06_eight_argument_fn_blocks_via_all_targets_gate
run case_07_fmt_judges_the_index_not_the_worktree
run case_08_worktree_edit_warns_but_does_not_block
run case_09_deleted_staged_rs_is_not_checked
run case_10_path_with_spaces_is_checked
run case_11_missing_cargo_fails_loudly
run case_12_missing_rustfmt_fails_loudly
run case_13_commit_from_subdirectory_is_gated
run case_14_hook_does_not_modify_the_tree
run case_15_webui_change_runs_both_webui_gates
run case_16_docs_name_the_installed_configuration

printf 'wall time: %ss\n' "$SECONDS"
if [ "$ok" -eq 16 ]; then
    printf 'PASS %d/16\n' "$ok"
else
    printf 'FAIL %d/16\n' "$ok"
    exit 1
fi
