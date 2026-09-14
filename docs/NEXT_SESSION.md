# Next-session handoff

Recorded: 2026-07-28

## User objective

Continue Pinky while sharply reducing paid-token expenditure. The direction to
evaluate next is a **Pinky Lite** cited-local-Q&A milestone followed by a
constrained offline self-development harness. See
[`OFFLINE_SELF_DEVELOPMENT.md`](OFFLINE_SELF_DEVELOPMENT.md).

Do not assume authorization to discard the complete version-one specification.
The offline route is a delivery strategy and sequencing change; the user should
confirm the pivot before implementation changes that remove or replace scope.

## Last committed state

Current `HEAD` when this handoff was written:

```text
6bbadef feat: add encrypted source retrieval and citations
```

That commit provides encrypted Tantivy lexical indexing, index rebuilding from
retained chunks, composer-based source search, and exact retained-version
`pinky://` citation reopening. Earlier commits provide encrypted vault setup and
restart unlocking, secure local text ingestion, stable-file watching, task
journaling, cancellation, process supervision, packaging checks, and the native
desktop shell.

## Uncommitted work that must be preserved

The working tree contains the next vector-retrieval foundation:

- `crates/pinky-core/src/hybrid.rs`: reciprocal-rank fusion with `k = 60`, chunk
  deduplication, and a three-chunk limit per source version;
- `crates/pinky-core/src/qdrant.rs`: supervised Qdrant launch configuration,
  random loopback ports, a fresh 256-bit API key, checked vault-resident storage,
  authenticated REST operations, cosine vectors, on-disk HNSW, scalar int8
  quantization, vector normalization, and shutdown through the existing process
  supervisor;
- dependency and export changes in `Cargo.lock`,
  `crates/pinky-core/Cargo.toml`, and `crates/pinky-core/src/lib.rs`;
- factual status changes in `README.md` and `docs/IMPLEMENTATION_STATUS.md`.

Do not overwrite, revert, or regenerate these changes. Inspect the live diff
before continuing because this handoff itself is also uncommitted.

## Verification already completed

Before this handoff was added:

- `cargo test --workspace`: 49 core tests and 3 desktop Rust tests passed;
- the opt-in live Secret Service/FUSE test remained ignored as designed;
- `cargo fmt --all` and `git diff --check` passed;
- Clippy was not run because the temporary Rust toolchain does not contain the
  `cargo-clippy` component.

The temporary toolchain used in this environment is:

```bash
PATH=/tmp/pinky-cargo/bin:$PATH \
RUSTUP_HOME=/tmp/pinky-rustup \
CARGO_HOME=/tmp/pinky-cargo \
cargo test --workspace
```

Run verification again after any subsequent edits. Do not claim a live Qdrant
acceptance test: no verified Qdrant executable or embedding model has been
installed yet.

## Recommended next decision

Ask the user to confirm one of these implementation directions before writing
the next runtime slice:

1. **Cost-first Pinky Lite:** connect an explicitly supplied loopback
   `llama-server`, generate cited answers from current lexical results, persist
   conversations, and defer model downloading plus Qdrant embeddings.
2. **Original delivery order:** finish signed model onboarding, verified model
   downloads, embeddings, Qdrant backfill, hybrid retrieval, and reranking
   before enabling cited chat.

The cost-first direction is recommended because it produces useful local Q&A
with fewer paid implementation sessions and creates the model runtime needed by
the later offline developer.

## Cost-first implementation sequence

If the user confirms the pivot:

1. Specify a strict loopback-only `llama-server` configuration and health check.
2. Add supervised launch or explicit attach mode with a per-launch token.
3. Send selected lexical passages through a source-grounded answer schema.
4. Reject factual sentences without mapped citations and expose unresolved
   gaps rather than inventing evidence.
5. Persist immutable encrypted conversations and messages.
6. Make inference cancellable and visible in the task panel.
7. Add native end-to-end coverage for a deterministic fake model server, then
   run a target-host smoke test with the chosen local GGUF.
8. Only then build the task-packet, snapshot, sandbox, test, retry, and
   escalation components described in the offline strategy.

## Commands for orientation

```bash
git status --short
git diff --check
git diff -- crates/pinky-core/src/hybrid.rs crates/pinky-core/src/qdrant.rs
sed -n '1,220p' docs/OFFLINE_SELF_DEVELOPMENT.md
sed -n '1,220p' docs/IMPLEMENTATION_STATUS.md
```

Do not commit unless the user explicitly asks for a commit.
