#!/usr/bin/env python3
"""Derive the 34-row consolidation plan from the rehearsal COPY only.

Inputs (all read-only reads on the copy):
  $S/projects/project_<hash>_shard_0.db   the shards
  $S/metadata.db                          the registry
Derivation:
  identity candidate from the shard's own association columns, CONFIRMED by
  sha256(identity)[:16] == the registry/scope hash; target identity by the
  LEADING-prefix swap /home/vaz -> /home/opencode (one pinned special case).
No implementation output is read.
"""
import hashlib, json, os, sqlite3, sys

S = sys.argv[1]                 # storage root of the copy
OUT = sys.argv[2]               # plan path
PREFIX = "opencode"
OLD, NEW = "/home/vaz", "/home/opencode"
SPECIAL = {}   # rev-12/13 + user ruling 2026-11-12: NO exceptions to the swap

def h16(s):
    return hashlib.sha256(s.encode()).hexdigest()[:16]

def ro(path):
    return sqlite3.connect(f"file:{path}?mode=ro", uri=True)

def assoc_of(shard):
    """(container_tag, project_path, project_name, git_repo_url) of one row."""
    con = ro(shard)
    try:
        row = con.execute(
            "SELECT container_tag, project_path, project_name, git_repo_url "
            "FROM memories LIMIT 1").fetchone()
    finally:
        con.close()
    return row

def identity_of(shard, want_hash):
    row = assoc_of(shard)
    if row is None:
        return None
    _tag, project_path, _project_name, git_repo_url = row
    candidates = []
    if project_path:
        candidates.append(f"path:{project_path}")
        candidates.append(f"git-common:{project_path}")
        # A git-common identity names the common dir, which sits one level
        # below the worktree the association column records.
        candidates.append(f"git-common:{project_path}/.git")
        candidates.append(f"path:{project_path}/.git")
    if git_repo_url:
        candidates.append(f"remote:{git_repo_url}")
    for cand in candidates:
        if h16(cand) == want_hash:
            return cand
    return None

def swap(identity):
    """LEADING-prefix swap only: the identity's path root, never mid-path."""
    scheme, _, path = identity.partition(":")
    if path == OLD or path.startswith(OLD + "/"):
        return f"{scheme}:{NEW}{path[len(OLD):]}"
    return identity

def association(target_identity, target_hash):
    path = target_identity.split(":", 1)[1]
    return {
        "container_tag": f"{PREFIX}_project_{target_hash}",
        "project_path": path,
        "project_name": os.path.basename(path),
        "display_name": path,
        "git_repo_url": None,
    }

projects = os.path.join(S, "projects")
shards = sorted(n for n in os.listdir(projects)
                if n.startswith("project_") and n.endswith("_shard_0.db"))

# Registry census: every row the copy's metadata.db carries.
con = ro(os.path.join(S, "metadata.db"))
registry = {r[0]: r[1] for r in con.execute(
    "SELECT scope_hash, db_path FROM shards WHERE scope='project'")}
con.close()

rows, skipped, unresolved = [], [], []
for name in shards:
    shard_hash = name.split("_")[1]
    path = os.path.join(projects, name)
    ident = identity_of(path, shard_hash)
    if ident is None:
        unresolved.append(name)
        continue
    if not (ident.split(":", 1)[1] == OLD or ident.split(":", 1)[1].startswith(OLD + "/")):
        skipped.append((name, ident))     # already-target / unrelated: not a source
        continue
    target_identity = SPECIAL.get(shard_hash, swap(ident))
    target_hash = h16(target_identity)
    rows.append({
        "source_hash": shard_hash,
        "source_identity": ident,
        "source_path": path,
        "target_hash": target_hash,
        "target_identity": target_identity,
        "target_path": os.path.join(projects, f"project_{target_hash}_shard_0.db"),
        "target_association": association(target_identity, target_hash),
        "allow_linked_source": False,
    })

# Registry cross-check: every source hash must be a registry row whose db_path
# names exactly this file (root-relative `<scope>s/<basename>`).
reg_ok = all(registry.get(r["source_hash"]) ==
             f"projects/{os.path.basename(r['source_path'])}" for r in rows)

# Schema manifest: my own digest over the copy's per-DB census.
def census(db):
    c = ro(db)
    try:
        uv = c.execute("PRAGMA user_version").fetchone()[0]
        tables = [r[0] for r in c.execute(
            "SELECT name FROM sqlite_master WHERE type='table' "
            "AND name NOT LIKE 'sqlite_%' ORDER BY name")]
        cols = {}
        for t in tables:
            cols[t] = [r[1] for r in c.execute(f"PRAGMA table_info('{t}')")]
        return uv, tables, cols
    finally:
        c.close()

lines = []
for name in sorted(os.listdir(S)):
    p = os.path.join(S, name)
    if not os.path.isfile(p) or not name.endswith(".db"):
        continue
    uv, tables, cols = census(p)
    lines.append(json.dumps([name, uv, tables, cols], sort_keys=True))
for name in shards:
    p = os.path.join(projects, name)
    uv, tables, cols = census(p)
    lines.append(json.dumps([f"projects/{name}", uv, tables, cols], sort_keys=True))
schema_manifest = hashlib.sha256(("\n".join(lines) + "\n").encode()).hexdigest()

# `consolidate-snapshot` writes `baseline.sha256` (file/dir format), not the
# `sha256  ./path` manifest this script's first revision assumed. Both are only
# bound into the plan digest — the CLI never verifies either against the tree —
# so hashing whichever the snapshot actually wrote is correct and needs no
# extra artifact in the window.
_custody = os.path.dirname(S)
for _name in ("manifest.sha256", "baseline.sha256"):
    _p = os.path.join(_custody, _name)
    if os.path.isfile(_p):
        baseline = hashlib.sha256(open(_p, "rb").read()).hexdigest()
        break
else:
    sys.exit(f"build_plan: no manifest.sha256 or baseline.sha256 beside {S}")

plan = {
    "format_version": 1,
    "storage_root": S,
    "source_snapshot_sha256": baseline,
    "schema_manifest_sha256": schema_manifest,
    "baseline_manifest_sha256": baseline,
    "mapping": rows,
}
fd = os.open(OUT, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "w") as fh:
    json.dump(plan, fh, indent=1)
    fh.write("\n")

print(json.dumps({
    "shards_on_disk": len(shards),
    "sources": len(rows),
    "unresolved": unresolved,
    "not_sources": [s[0] for s in skipped],
    "registry_ok": reg_ok,
    "schema_manifest_sha256": schema_manifest,
    "baseline_manifest_sha256": baseline,
    "plan_mode": oct(os.stat(OUT).st_mode & 0o777),
}, indent=1))
