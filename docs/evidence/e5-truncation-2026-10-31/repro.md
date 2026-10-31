# e5-base truncation failure — reproduction transcript

**Date:** 2026-10-31 (local ~22:55 +07:00 / 15:55Z)
**Why:** the live `--swap` to `Xenova/multilingual-e5-base` q8 stopped on scope 2 of 22. This is the
transcript of the reproduction that identified the cause: `encode_text` does not truncate, and
e5-base's ONNX graph rejects inputs beyond its 512-token position limit.

All labels are MEASURED unless stated. Nothing here wrote to the live store: the reproduction ran
against a `cp -a --reflink=auto` copy with its `.engine.lock` and `status` removed.

---

## 1. The live failure, from the executor's own log

`~/.opencode-mem-backups/adopt.log` (MEASURED, verbatim lines):

```
[2026-10-31T15:50:08Z] re-embedding 22 of 22 scopes
[2026-10-31T15:50:08Z] re-embedding opencode_project_03b7931d97d0837c
[2026-10-31T15:50:15Z] re-embedded opencode_project_03b7931d97d0837c (re_embedded=63)
[2026-10-31T15:50:15Z] re-embedding opencode_project_12f3367ad73f86b8
[2026-10-31T15:50:18Z] re-embed FAILED for opencode_project_12f3367ad73f86b8 (rc 74)
```

The executor stopped on the first failure, as designed. The config had already been flipped to e5,
so the store was left mixed (1 scope e5, 21 nomic) and cross-shard search answered HTTP 409
`embedding model mismatch`.

## 2. The reproduction (scratch copy)

```sh
R=$PWD/.tmp/scratch/repro-scope2
cp -a --reflink=auto ~/.opencode-mem/data "$R/data"
rm -f "$R/data/.engine.lock" "$R/data/status"
sed "s|\"storagePath\": \"~/.opencode-mem/data\"|\"storagePath\": \"$R/data\"|" \
  ~/.config/opencode/opencode-mem.jsonc > "$R/home/.config/opencode/opencode-mem.jsonc"
HOME="$R/home" timeout 300 plugin/engine/linux-x64/opencode-agents-memory \
  --mode=reembed-scope --scope opencode_project_12f3367ad73f86b8 --storage-root "$R/data"
```

Output (MEASURED, verbatim):

```
2026-10-31 22:55:43.529667724 [E:onnxruntime:, sequential_executor.cc:677 ExecuteKernel] Non-zero status code returned while running Expand node. Name:'/Expand' Status Message: invalid expand shape
{"duration_ms":4134,"event":"reembed.completed","re_embedded_memories":0,"strategy":"re-embed","success":false}
{"component":"reembed","detail":"ONNX inference failed: Non-zero status code returned while running Expand node. Name:'/Expand' Status Message: invalid expand shape","event":"reembed.failed","level":"error"}
rc=74
```

Deterministic: the same command reproduces the same `Expand` failure in ~4 s.

## 3. The model's declared limit

`~/.opencode-mem/data/.cache/Xenova/multilingual-e5-base/config.json` (MEASURED):

```
"max_position_embeddings": 514     -> 512 usable positions after the template's 2 specials
"model_type": "xlm-roberta"
"hidden_size": 768, "num_hidden_layers": 12, "num_attention_heads": 12
```

By contrast, `~/.opencode-mem/data/.cache/Xenova/nomic-embed-text-v1/config.json` (MEASURED):

```
"max_position_embeddings": null    -- ABSENT
"n_positions": 8192
```

So the current model declares its limit under a **different key** than the model that failed. A
reader that checks only `max_position_embeddings` returns `None` for the model in production.

## 4. The corpus, and how much of it exceeds the limit

MEASURED over the scratch copy (`projects/*.db`, `select length(content)`):

```
rows store-wide:                3,079
rows with length(content) > 1800 chars:   213
```

The failing scope, `project_12f3367ad73f86b8_shard_0.db` (57 rows), longest contents:

```
2310  muse_bf16_proj (2026-10-28, ADR .tmp/drafts/adr-20261028-resume-chunke...
1727  muse_bf16_proj (2026-10-28, ADR .tmp/drafts/adr-20261028-pack-or-pivot...
1460  STOPPED the muse_bf16_proj kernel track at the banked 1.0921x (job 3c5...
1209  codemap skill (id codemap, /home/vaz/.config/opencode/skills/codemap) ...
1110  E2 chunk-pack probe APPROVED (blueprint-20261028-chunkpack-probe-v3, b...
1063  E1 chunk-wave blueprint APPROVED (blueprint-20261028-chunkwave-v4, bod...
```

The 2,310-char row is the one that trips the 512-token limit.

**INFERRED, not measured:** the `> 1800 chars` threshold is a prose approximation of 512 tokens
(~3.5 chars/token). Byte-level tokenizers can exceed one token per character on Thai, so the true
count of over-limit rows is **at least** 213 and can only be established by tokenizing each row.
The 512-position limit itself is MEASURED (config + the graph's own rejection).

## 5. What this does and does not prove

- **Proven:** the e5 graph rejects an input longer than its position table; the port never
  truncates; therefore e5 cannot be adopted until inputs are capped.
- **Proven:** nomic declares its limit as `n_positions: 8192`, not `max_position_embeddings`.
- **NOT proven here:** that nomic enforces 8,192 positions on every path (INFERRED from the config
  and from the corpus maximum of 6,603 chars, which is below it).
- **NOT proven here:** the +635 MB RSS cost of a 12,000-char `add` — that measurement lives in
  `.tmp/progress/engine-rss-debug.md` §3c/§4 H5, on a scratch probe.
