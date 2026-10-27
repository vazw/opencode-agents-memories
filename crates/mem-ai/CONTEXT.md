# mem-ai

**Purpose:** AI provider seam: `AiProvider` trait + provider clients + privacy filtering before anything is persisted or sent.

**Ubiquitous language:**

- **private tag** — the `<private>` / `</private>` markup whose region must never leave the process. Grammar: `<`, optional `/`, ASCII-case-insensitive `private`, zero-or-more *tag whitespace* characters, `>`. Anything else (`<private/>`, `<private mode="strict">`, `<privateer>`) is ordinary text.
- **tag whitespace** — `char::is_whitespace()` **or** U+FEFF. The two parity boundaries against JS `\s` are deliberate: U+FEFF must be recognized (JS matches it, Rust's `White_Space` does not, so a bare predicate would leak), U+0085 is accepted (JS rejects it, Rust accepts it — errs toward redaction).
- **redaction** — replacing a private region (or an unclosed one, to EOF) with `[REDACTED]` (`mem_ai::privacy::REDACTED`). Over-redaction is the only safe error direction.
- **fully private** — nothing survives redaction after trimming (`is_fully_private`); such content is refused by the write path rather than stored.
- **injected part** — a message part flagged `synthetic` by the host, or whose text carries an injection marker. Never recorded as if the user had typed it.
- **internal prompt** — one of the plugin's *own* prompts (`# User Profile Analysis`, or `Analyze this conversation.` together with `type="skip"`). Exempt from the injection blocklist; import `is_internal_prompt` rather than re-porting it (issue 52 consumes it at the capture seam).
- **secret reference** — `env://NAME`, `file://PATH` or a bare literal; `SecretSource::parse` decides which.
- **permissive mode** — a secret file with *any* group or other permission bit set (`group_or_other_bits(mode) != 0`). Strict rejection, not upstream's warning.
- **privacy layer** — `PrivacyFilterLayer`, the `AiProvider` middleware that applies the stripper and the injection filter to an arbitrary wrapped provider.
- **request-side filtering** — the layer filters only what it *sends* (completion `messages` + `system`, embed inputs, `count_tokens` text, the structured request). Responses — chunks, vectors, counts, model lists, structured values — pass through verbatim.

**Deferred seams:** chunk-incremental stripping (`push`/`finish`), config→resolver wiring (the bootstrap wave constructs `FileSecretResolver`), per-call secret re-read caching, and a richer message for owner-unreadable-but-exposed secret files.
