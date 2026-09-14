# Next-session handoff

Recorded: 2026-09-14

## Active objective

Deliver the cost-first Pinky Lite milestone: useful local question answering
over encrypted retained sources, with exact citations, visible cancellation,
and encrypted conversations. The complete version-one specification remains
the longer-term scope; embeddings, Qdrant, web research, and generation are not
prerequisites for Pinky Lite.

The authoritative delivery sequence, contracts, and gates are in
[`CITED_QA_PHASES.md`](CITED_QA_PHASES.md).

## Phase baseline before R3

```text
7f13b52 feat: add bounded Ollama inference transport
```

That baseline includes the encrypted retained-evidence foundation, explicit
Ollama attachment, and bounded cancellable structured inference transport.

## Current completed phase

R3 implements the strict evidence and answer contract in
`crates/pinky-core/src/qa.rs`. It bounds and deduplicates lexical evidence,
regenerates Pinky-owned citation IDs, quotes source text as untrusted data,
requires every factual summary and claim to cite supplied evidence, and permits
only one bounded repair attempt. Empty retrieval returns an explicit gap without
model inference. Generated text is still neither displayed nor persisted.

## Verification completed

- `cargo fmt --all --check`: passed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- `cargo test --workspace`: 83 core and 6 desktop tests passed
- two opt-in live Ollama target-host tests: passed separately
- opt-in live Secret Service/FUSE and Ollama tests: ignored in the ordinary
  regression suite as designed
- `npm test`: 3 frontend tests passed
- `npm run build`: passed without warnings after deterministic vendor chunking
- `npm run test:e2e`: passed
- `git diff --check`: passed

The deterministic fake llama-server test needs loopback permission; the
restricted filesystem sandbox returned `EPERM`, and the same suite passed when
run with explicit local-loopback permission.

## Next implementation slice: R4 visible one-shot cited answers

Wire the accepted R3 contract into the desktop without adding persistence yet:

1. Add explicit Ask and Search modes with honest enablement rules.
2. Run retrieval and validated inference as one visible cancellable task.
3. Render the summary, claims, qualifications, gaps, and clickable exact
   citations only after R3 validation succeeds.
4. Prevent cancellation, timeout, tunnel loss, and invalid model output from
   producing a completed or late answer.
5. Pass the frontend, native, and target-host tutorial gates in
   `CITED_QA_PHASES.md`.

## Orientation commands

```bash
git status --short
git diff --check
git log -5 --oneline
sed -n '1,260p' docs/CITED_QA_PHASES.md
/home/mickey/.cargo/bin/cargo test --workspace
```

Do not commit unless the user explicitly asks for a commit.
