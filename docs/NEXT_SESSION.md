# Next-session handoff

Recorded: 2026-09-15

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
model inference. Generated text is still not persisted as conversation history.

R4 is now implemented on the Ollama-first route. The desktop has explicit
Ask/Search modes; Ask requires an unlocked vault, at least one retained source,
and an attached model. Retrieval, evidence assembly, local inference, and R3
validation run in one cancellable task. Validated summaries and claims render
with warnings, unresolved gaps, and exact retained-snapshot citation links.
Invalid, cancelled, detached, timed-out, or unavailable model requests remain
errors or gaps and cannot update the UI after a newer request. The target-host
tutorial demonstration is still an opt-in acceptance check.

R5 is now implemented for local cited Q&A. Conversation and message bodies are
stored as compressed content-addressed objects inside the encrypted vault;
metadata is ordered and immutable in SQLCipher. The desktop restores chats
after unlock, sends only the bounded recent history to the model, persists an
assistant message only after validation, and supports encrypted create/select/
rename/delete interactions. The direct filesystem plaintext inspection remains
an opt-in release check.

R6 reliability implementation is now complete: interrupted task journals
recover as `failed_interrupted`, recoverable model transport failures receive
one bounded retry without duplicate assistant messages, and the Ask error path
offers local-model reconnect guidance. Existing encrypted task journals retain
bounded diagnostics; keyboard/live-region, reduced-motion, and non-WebGL paths
remain enabled. A deterministic offline acceptance test now covers tutorial
ingestion, cited retrieval, exact citation reopening, evidence gaps,
pre-cancellation, and encrypted conversation restart. The pinned clean-Debian
package and fresh-profile model-offline checks pass. The configured target-host
offline demonstration and direct plaintext inspection now pass. The real
Ollama scenario and bounded external-profile scanner are wired in
`tests/live_ollama.rs` and `docs/PRIVACY_INSPECTION.md`; only the
SSH-specific process-boundary review requires a target-host run with the
user's credentials.

## Verification completed

- `cargo fmt --all --check`: passed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- `cargo test --workspace`: 94 core unit, 1 offline integration, and 10 desktop
  tests passed
- two opt-in live Ollama target-host tests: passed separately
- opt-in live Secret Service/FUSE and Ollama tests: ignored in the ordinary
  regression suite as designed
- `npm test`: 12 frontend tests passed
- `npm run build`: passed without warnings after deterministic vendor chunking
- `npm run test:e2e`: passed, including locked Ask/Search mode and task controls
- `cargo test -p pinky-core --test pinky_lite_offline`: passed, including the
  offline tutorial cited-answer and encrypted conversation restart scenario
- `npm run verify:bundle`: passed for the Debian and AppImage artifacts
- `npm run test:clean-install`: passed in the pinned Debian container
- `node apps/desktop/scripts/inspect-plaintext.mjs`: passed with a positive
  external-marker and excluded-vault fixture
- R6 target-host Ollama acceptance: the three ignored live checks passed on
  2026-09-15 against `qwen3.5:4b` through an explicit loopback endpoint. A
  temporary local forward was used because SSH credentials were unavailable in
  this shell; repeat the SSH-specific process-boundary review on the target
  host before release.
- R6 direct plaintext inspection: passed against the application-data and cache
  roots with the mounted vault excluded; no configured evidence markers were
  found outside the vault. The positive/negative scanner fixtures also pass.
- `git diff --check`: passed

The deterministic fake llama-server test needs loopback permission; the
restricted filesystem sandbox returned `EPERM`, and the same suite passed when
run with explicit local-loopback permission.

## Next implementation slice: R7 onboarding and release acceptance

Run the remaining SSH-specific process-boundary check described in
`CITED_QA_PHASES.md`, then continue with R7 onboarding and release acceptance.
R7 now includes the bounded embedding
provider/indexer contract, current retained-chunk loading for backfill,
citation-preserving fusion, a separate Ollama embedding-model probe with a
smoke vector, and cancellable core hybrid-search orchestration. An opt-in
desktop bridge now validates the embedding model, lazily starts and reuses one
supervised Qdrant sidecar per application session, and reports retrieval
phases when the three documented environment values are set. The runtime panel
now verifies and stores the hybrid settings inside the encrypted vault and
stops the sidecar after five idle minutes. The signed artifact manifest
verifier now covers strict metadata, Ed25519 signatures, checksums, streamed
downloads with redirects rejected, and atomic installation. The core reranker
now bounds hybrid selection to 30 fused candidates and 12 returned citations.
Use `docs/PRIVACY_INSPECTION.md` for the scanner command and run
the ignored live test with `PINKY_OLLAMA_ENDPOINT` and
`PINKY_OLLAMA_MODEL` set. The remaining R7 release gate is target-host
onboarding/backfill with a verified Qdrant executable and embedding model,
followed by the one-million-chunk warm p95 measurement.

Product priority change: R8 image metadata ingestion is implemented for PNG,
JPEG, WebP, GIF, and TIFF. A bounded supervised OCR worker now keeps its
materialized input and output inside the mounted vault, attaches normalized OCR
chunks to the current source version, and cleans up on completion or
cancellation. Image citations can explicitly load the exact retained pixels.
Deterministic cancellation and vault-reopen acceptance fixtures now pass, so R8
is complete for this retained-image scope.
Image generation is deferred to a later creative-tools phase. PDF and Office
extraction are deferred until after R8; do not start those extractors as the
next slice.

## Orientation commands

```bash
git status --short
git diff --check
git log -5 --oneline
sed -n '1,260p' docs/CITED_QA_PHASES.md
/home/mickey/.cargo/bin/cargo test --workspace
```

Do not commit unless the user explicitly asks for a commit.
