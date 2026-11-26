#!/usr/bin/env python3
"""Language census over an opencode-mem store copy. Read-only, stdlib only.

Usage: census.py <store-root>
  <store-root> must be a *copy* of a data directory (projects/*.db + user-prompts.db).
  Every database is opened with mode=ro. Nothing is ever written.

Why this exists: issue 167 proposed swapping the embedding model from
Xenova/nomic-embed-text-v1 to Xenova/multilingual-e5-base on the strength of a
Thai nDCG@10 number. This census answers the prior question — is there any
non-Latin-script content in the store at all? If not, the swap buys nothing on
the corpus axis, and its 512-token cap (nomic declares 8192 positions) is pure
cost. The census is re-runnable evidence, not a gate that runs itself.

Density rule (applied by the gates): a row counts as non-Latin script only when
a single script category supplies >= 5 % of the row's non-whitespace characters.
Reason: Greek letters in this store are mathematical notation (delta, sigma,
lambda, micro), not Greek prose; em dashes and arrows are typography, not
language. A naive "contains any" gate would sit at ~37 rows today and drift
with every extra delta. The 5 % floor makes one symbol in a sentence irrelevant
and a real script-bearing sentence impossible to miss.

Exit status is always 0: a tripped gate is a measurement result, not an error.
"""

import glob
import os
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path

# Characters the model actually sees, grouped by script/typographic family.
CATEGORIES = [
    ("latin_accented", [(0x00C0, 0x024F)]),
    ("thai",           [(0x0E00, 0x0E7F)]),
    ("han",            [(0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xF900, 0xFAFF)]),
    ("kana",           [(0x3040, 0x30FF)]),
    ("hangul",         [(0xAC00, 0xD7AF)]),
    ("cyrillic",       [(0x0400, 0x04FF)]),
    ("arabic",         [(0x0600, 0x06FF)]),
    ("hebrew",         [(0x0590, 0x05FF)]),
    ("devanagari",     [(0x0900, 0x097F)]),
    ("greek",          [(0x0370, 0x03FF)]),
    ("punctuation",    [(0x2000, 0x206F), (0x00A0, 0x00BF)]),
    ("symbols_math",   [(0x2190, 0x23FF), (0x25A0, 0x27BF), (0x2900, 0x297F)]),
    ("emoji",          [(0x1F000, 0x1FAFF)]),
]

# Scripts that count for the gates. latin_accented is Western European
# orthography, not a different language; punctuation/symbols_math/emoji are
# typography; greek is excluded for the notation reason in the docstring.
GATE_SCRIPTS = ("thai", "han", "kana", "hangul", "cyrillic",
                "arabic", "hebrew", "devanagari")
GATE_ORDER = [name for name, _ in CATEGORIES if name in GATE_SCRIPTS]
DENSITY_FLOOR = 0.05
GATE_THRESHOLD = 0.01   # 1 % for memory_share and prompt_share
PROJECT_THRESHOLD = 0.50  # 50 % for project_majority

# Flat lookup: (lo, hi, category) sorted for a simple linear scan. Corpus is
# small (thousands of rows), so clarity beats a bisect or a translation table.
RANGES = [(lo, hi, name) for name, ranges in CATEGORIES for lo, hi in ranges]


def classify(ch):
    cp = ord(ch)
    if cp < 0x80:
        return None
    for lo, hi, name in RANGES:
        if lo <= cp <= hi:
            return name
    return None  # non-ASCII but outside every studied range


def scan_text(text):
    """Return (per-category char counts, non-whitespace char count)."""
    counts = {name: 0 for name, _ in CATEGORIES}
    nonws = 0
    for ch in text:
        if not ch.isspace():
            nonws += 1
        cat = classify(ch)
        if cat is not None:
            counts[cat] += 1
    return counts, nonws


def gate_script_density(counts, nonws):
    """True when one gate script supplies >= 5 % of non-whitespace characters."""
    if nonws == 0:
        return False, 0.0
    best = max((counts[name] for name in GATE_SCRIPTS), default=0)
    share = best / nonws
    return share >= DENSITY_FLOOR, share


def row_stats(text):
    counts, nonws = scan_text(text)
    gate_raw = sum(counts[name] for name in GATE_SCRIPTS) > 0
    gate_dense, _ = gate_script_density(counts, nonws)
    return {
        "counts": counts,
        "nonws": nonws,
        "any_non_ascii": any(ord(ch) >= 0x80 for ch in text),
        "gate_raw": gate_raw,
        "gate_dense": gate_dense,
    }


def open_ro(path):
    return sqlite3.connect(f"file:{path}?mode=ro", uri=True)


def pct(part, whole):
    return 0.0 if whole == 0 else 100.0 * part / whole


def iso(ms):
    return datetime.fromtimestamp(ms / 1000, tz=timezone.utc).isoformat()


def refuse_live(store_root):
    """Never read the live store, even by accident. Exit 0 per contract."""
    resolved = Path(store_root).resolve()
    live = Path.home() / ".opencode-mem"
    try:
        resolved.relative_to(live)
    except ValueError:
        return False
    print(f"REFUSED: {resolved} is inside the live store ({live}).")
    print("Copy the store first; this tool is read-only but the input must be a copy.")
    return True


def census_memories(store_root):
    shard_paths = sorted(glob.glob(os.path.join(store_root, "projects", "*.db")))
    total_rows = 0
    ascii_rows = 0
    nonascii_rows = 0
    raw_rows = 0
    dense_rows = 0
    char_totals = {name: 0 for name, _ in CATEGORIES}
    row_totals = {name: 0 for name, _ in CATEGORIES}
    long_1k8 = 0
    long_4k = 0
    longest = 0
    byte_char_mismatch = 0
    per_project = []

    for path in shard_paths:
        conn = open_ro(path)
        try:
            rows = conn.execute("SELECT content FROM memories").fetchall()
            # Byte-vs-character cross-check: SQLite length() counts bytes for a
            # BLOB and characters for TEXT. A mismatch means multi-byte UTF-8.
            bb = conn.execute(
                "SELECT COUNT(*) FROM memories "
                "WHERE length(cast(content as blob)) > length(content)"
            ).fetchone()[0]
        finally:
            conn.close()

        byte_char_mismatch += bb
        proj_rows = len(rows)
        proj_dense = 0
        for (content,) in rows:
            st = row_stats(content)
            total_rows += 1
            if st["any_non_ascii"]:
                nonascii_rows += 1
            else:
                ascii_rows += 1
            if st["gate_raw"]:
                raw_rows += 1
            if st["gate_dense"]:
                dense_rows += 1
                proj_dense += 1
            for name, n in st["counts"].items():
                char_totals[name] += n
                if n:
                    row_totals[name] += 1
            n = len(content)
            if n > 1800:
                long_1k8 += 1
            if n > 4096:
                long_4k += 1
            if n > longest:
                longest = n
        per_project.append((os.path.basename(path), proj_rows, proj_dense))

    return {
        "shards": len(shard_paths),
        "total_rows": total_rows,
        "ascii_rows": ascii_rows,
        "nonascii_rows": nonascii_rows,
        "raw_rows": raw_rows,
        "dense_rows": dense_rows,
        "char_totals": char_totals,
        "row_totals": row_totals,
        "long_1k8": long_1k8,
        "long_4k": long_4k,
        "longest": longest,
        "byte_char_mismatch": byte_char_mismatch,
        "per_project": per_project,
    }


def census_prompts(store_root):
    path = os.path.join(store_root, "user-prompts.db")
    conn = open_ro(path)
    try:
        rows = conn.execute(
            "SELECT content, created_at FROM user_prompts"
        ).fetchall()
        bb = conn.execute(
            "SELECT COUNT(*) FROM user_prompts "
            "WHERE length(cast(content as blob)) > length(content)"
        ).fetchone()[0]
    finally:
        conn.close()

    total = len(rows)
    ascii_rows = 0
    nonascii_rows = 0
    raw_rows = 0
    dense_rows = 0
    char_totals = {name: 0 for name, _ in CATEGORIES}
    row_totals = {name: 0 for name, _ in CATEGORIES}
    stamps = []
    for content, created_at in rows:
        st = row_stats(content)
        if st["any_non_ascii"]:
            nonascii_rows += 1
        else:
            ascii_rows += 1
        if st["gate_raw"]:
            raw_rows += 1
        if st["gate_dense"]:
            dense_rows += 1
        for name, n in st["counts"].items():
            char_totals[name] += n
            if n:
                row_totals[name] += 1
        stamps.append(created_at)

    return {
        "total_rows": total,
        "ascii_rows": ascii_rows,
        "nonascii_rows": nonascii_rows,
        "raw_rows": raw_rows,
        "dense_rows": dense_rows,
        "char_totals": char_totals,
        "row_totals": row_totals,
        "byte_char_mismatch": bb,
        "created_min": min(stamps) if stamps else None,
        "created_max": max(stamps) if stamps else None,
    }


def print_category_table(char_totals, row_totals, total_rows, title):
    print(title)
    print(f"  {'category':<16}{'rows':>8}{'row%':>9}{'chars':>10}")
    for name, _ in CATEGORIES:
        print(f"  {name:<16}{row_totals[name]:>8}"
              f"{pct(row_totals[name], total_rows):>8.2f}%{char_totals[name]:>10}")


def main(argv):
    if len(argv) != 2:
        print("usage: census.py <store-root>")
        return 0
    store_root = argv[1]
    if refuse_live(store_root):
        return 0
    if not os.path.isdir(os.path.join(store_root, "projects")):
        print(f"no projects/ directory under {store_root}; is that a store copy?")
        return 0

    try:
        m = census_memories(store_root)
        p = census_prompts(store_root)
    except sqlite3.Error as exc:
        # Exit 0 is part of the contract; a broken input still prints loudly.
        print(f"ERROR: {exc}")
        return 0

    print(f"store: {os.path.abspath(store_root)}")
    print(f"shard dbs: {m['shards']}")
    print("")
    print("=== memory corpus (memories.content) ===")
    print(f"total rows              {m['total_rows']}")
    print(f"fully ASCII rows        {m['ascii_rows']}"
          f"  ({pct(m['ascii_rows'], m['total_rows']):.2f} %)")
    print(f"rows with >=1 non-ASCII {m['nonascii_rows']}"
          f"  ({pct(m['nonascii_rows'], m['total_rows']):.2f} %)")
    print(f"byte-vs-char cross-check {m['byte_char_mismatch']} rows"
          " (length(blob) > length(text))")
    print("")
    print_category_table(m["char_totals"], m["row_totals"], m["total_rows"],
                         "per-category counts (all rows):")
    print("")
    print("gate-script rows, raw 'contains any': "
          f"{m['raw_rows']} ({pct(m['raw_rows'], m['total_rows']):.2f} %)")
    print(f"gate-script rows, density >= {DENSITY_FLOOR:.0%}: "
          f"{m['dense_rows']} ({pct(m['dense_rows'], m['total_rows']):.2f} %)")
    print("")
    print("per-project (density-filtered non-Latin-script share):")
    print(f"  {'shard':<52}{'rows':>8}{'script':>8}{'share':>9}")
    worst_share = 0.0
    for name, rows, dense in m["per_project"]:
        share = pct(dense, rows)
        if share > worst_share:
            worst_share = share
        print(f"  {name:<52}{rows:>8}{dense:>8}{share:>8.2f}%")
    print("")
    print(f"length: rows > 1800 chars {m['long_1k8']}"
          f" ({pct(m['long_1k8'], m['total_rows']):.2f} %),"
          f" rows > 4096 chars {m['long_4k']}, longest {m['longest']} chars")
    print("")
    print("=== prompt corpus (user-prompts.db / user_prompts.content) ===")
    print(f"total rows              {p['total_rows']}")
    print(f"fully ASCII rows        {p['ascii_rows']}"
          f"  ({pct(p['ascii_rows'], p['total_rows']):.2f} %)")
    print(f"rows with >=1 non-ASCII {p['nonascii_rows']}"
          f"  ({pct(p['nonascii_rows'], p['total_rows']):.2f} %)")
    print(f"byte-vs-char cross-check {p['byte_char_mismatch']} rows"
          " (length(blob) > length(text))")
    print("")
    print_category_table(p["char_totals"], p["row_totals"], p["total_rows"],
                         "per-category counts (all prompts):")
    print("")
    print("gate-script rows, raw 'contains any': "
          f"{p['raw_rows']} ({pct(p['raw_rows'], p['total_rows']):.2f} %)")
    print(f"gate-script rows, density >= {DENSITY_FLOOR:.0%}: "
          f"{p['dense_rows']} ({pct(p['dense_rows'], p['total_rows']):.2f} %)")
    if p["created_min"] is not None:
        print(f"created_at range        {iso(p['created_min'])} .. {iso(p['created_max'])}")
    print("")
    print("=== gate ===")
    memory_share = pct(m["dense_rows"], m["total_rows"])
    project_share = worst_share
    prompt_share = pct(p["dense_rows"], p["total_rows"])
    print(f"GATE memory_share: {memory_share:.2f} % \u2014 "
          f"{'TRIPPED' if memory_share >= 100 * GATE_THRESHOLD else 'clear'}")
    print(f"GATE project_majority: {project_share:.2f} % \u2014 "
          f"{'TRIPPED' if project_share >= 100 * PROJECT_THRESHOLD else 'clear'}")
    print(f"GATE prompt_share: {prompt_share:.2f} % \u2014 "
          f"{'TRIPPED' if prompt_share >= 100 * GATE_THRESHOLD else 'clear'}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
