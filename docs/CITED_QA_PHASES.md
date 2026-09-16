# Cited local Q&A development plan

Status: active, cost-first delivery contract.

This plan starts from Pinky's encrypted source search and attached Ollama
runtime. Its first objective is narrow but useful: ask a question about approved
local text sources and receive a local answer whose citations reopen the exact
retained passages.

Model downloads, supervised `llama-server`, embeddings, Qdrant, document
extraction, and web research are not prerequisites for the first usable
release. A checked item does not complete a phase until every gate for that
phase passes.

## Delivery checkpoints

| Checkpoint | User-visible outcome | Completed after |
| --- | --- | --- |
| Connected | Pinky validates an explicitly selected local model | R1 |
| First useful | Pinky answers from retained text with clickable citations | R4 |
| Persistent | Validated conversations survive restart in the vault | R5 |
| Pinky Lite | Offline, failure, cancellation, and privacy gates pass | R6 |

## Global contracts

### Privacy and transport

- Pinky connects only to an explicit `http://127.0.0.1:<port>` model endpoint
  and bypasses ambient HTTP proxies.
- A model on another user-controlled machine is reached only through a
  user-established encrypted loopback tunnel such as SSH. Pinky does not scan
  the LAN, connect directly to LAN addresses, or manage SSH credentials.
- Ollama models with `remote_model` or `remote_host` metadata are rejected.
  Direct cloud endpoints and cloud-backed inference are out of scope.
- Prompts, passages, responses, and conversations are never written outside a
  verified mounted vault. Diagnostics must not contain source or request text.

### Evidence and model trust

- Source text and model output are untrusted data. Neither can change system
  instructions, permissions, limits, citation IDs, or tool schemas.
- The model receives only the question, selected evidence, bounded conversation
  context, and required output schema.
- Pinky generates citation identifiers. The model may select supplied IDs but
  may not invent or rewrite them.
- Every displayed factual claim has validated citations or is explicitly
  labelled as inference, disputed, or unresolved.
- Insufficient evidence produces a gap response, never a general-knowledge
  fallback.

### Operations and acceptance

- Retrieval and inference are represented by durable task events.
- Cancellation is acknowledged immediately; partial output is never presented
  as complete.
- Protocol parsing has bounded sizes, timeouts, strict required fields, and
  rejection of unknown answer-schema fields.
- Every defined automated gate requires a 100 percent pass rate.
- Existing vault, ingestion, retrieval, citation, task, accessibility, and
  packaging behavior must not regress.

## R0 — retained-evidence baseline

Status: complete.

Contract: approved text sources can be encrypted, versioned, searched, and
reopened through exact retained-version citations.

- [x] Encrypted local ingestion and stable-file watching
- [x] Current-version Tantivy lexical retrieval
- [x] Exact retained-version citation viewer
- [x] Durable task events and cancellation primitives

Gate: existing Rust, frontend, native, vault, and citation suites pass.

## R1 — explicit local model boundary

Status: complete for the Ollama-first delivery track. Managed llama.cpp launch
remains a non-blocking compatibility track.

Contract: after vault unlock, the user can attach one validated inference
runtime without persisting credentials or accessing an unapproved destination.

### Ollama path

- [x] Accept only an exact IPv4-loopback origin with an explicit port
- [x] Follow Ollama's local no-key API and send no authorization header
- [x] Probe `/api/version`, `/api/tags`, and `/api/show`
- [x] Require an installed, non-remote GGUF completion model
- [x] Require at least 2,048 advertised context tokens
- [x] Surface provider, model, context, progress, and errors
- [x] Drop the client on detach or vault loss
- [x] Cover endpoint, no-auth, cloud, format, context, and cancellation behavior
  with deterministic core tests

### llama.cpp compatibility path

- [x] Require a 256-bit hexadecimal bearer token for explicit attach
- [x] Keep the token process-only and redact it from diagnostics
- [x] Validate `/health`, protected `/props`, model, slots, and context
- [ ] Launch a supplied `llama-server` under process supervision
- [ ] Generate a fresh token and random port for every managed launch
- [ ] Prove managed shutdown leaves no descendant process

R1 gate:

- [x] Native desktop command-state tests cover locked vault, concurrent attach,
  successful attach, detach, failure, and vault loss; core fixtures cover
  malformed responses and cancellation.
- [x] The opt-in target-host test passed on 2026-09-14 through
  `http://127.0.0.1:11435` against Ollama 0.33.2 and `qwen3.5:9b`, advertising
  262,144 context tokens.
- [x] Deterministic llama.cpp tests remain green. Managed llama.cpp launch is a
  compatibility sub-gate and does not block the Ollama-first R2-R6 route.

## R2 — cancellable Ollama generation transport

Status: complete.

Contract: Pinky submits a bounded structured-output request to the attached
Ollama model and returns an untrusted response to the validator. Generated text
is not displayed or persisted yet.

Ollama's documented [`/api/chat`](https://docs.ollama.com/api/chat) request
accepts a JSON Schema in `format`; Pinky still validates the returned content
itself because runtime schema enforcement does not make model output trusted.

- [x] Add a provider-neutral inference trait
- [x] Implement Ollama `POST /api/chat` with `stream: false` for the initial
  structured-output slice
- [x] Supply a JSON Schema in `format`, deterministic generation options, and
  an explicit bounded `keep_alive`
- [x] Verify the response model and reject remote/cloud response metadata
- [x] Bound request size, response size, connection time, inference time, and
  retained error bodies
- [x] Support cooperative cancellation and discard late responses
- [x] Return timing and token counts without logging request or response bodies
  outside the vault
- [x] Distinguish unavailable server, missing model, timeout, cancellation,
  malformed response, and remote response errors

R2 gate:

- [x] Fake-Ollama tests cover exact request shape, schema, absence of authorization
  and proxy use, success, defined errors, cancellation, late responses, and
  size limits.
- [x] On 2026-09-14, the target-host smoke test submitted a non-sensitive fixed
  schema through the SSH tunnel to `qwen3.5:9b` and validated the response.

## R3 — evidence and answer contract

Status: complete.

Contract: retrieval occurs first; Pinky accepts only a structured answer whose
claims map to evidence supplied for that exact request.

### Versioned request

`QuestionRequestV1` contains:

- schema version, task UUID, and question;
- provider and model identity;
- evidence entries with immutable citation URI, display name, coordinates,
  retrieval timestamp, and passage;
- evidence and output limits; and
- explicit untrusted-evidence delimiters.

### Versioned answer

`AnswerEnvelopeV1` contains only:

- `summary`: a concise direct response;
- `summary_citations`: supplied evidence IDs supporting the direct response;
- `claims`: ordered `statement`, `support`, and `citations` objects;
- `warnings`: stale, disputed, or single-source qualifications; and
- `unresolved_gaps`: matters the supplied evidence could not answer.

`support` is `direct`, `inference`, or `disputed`. Direct and disputed
claims require supplied citations. Inferences require cited premises and
explicit inference wording. Unknown fields, empty or duplicate claims, invented
citations, and unbounded strings or arrays are rejected.

### Retrieval and prompt assembly

- [x] Retrieve only current versions through the existing lexical index
- [x] Deduplicate chunks and cap each source version at three chunks
- [x] Select at most 12 chunks and 8,000 approximate context tokens
- [x] Preserve contradictory passages
- [x] Delimit evidence as untrusted quoted material
- [x] Supply only Pinky-generated citation IDs
- [x] Validate output independently of Ollama's schema enforcement
- [x] Permit at most one bounded repair request
- [x] Return an evidence gap without inference when retrieval is empty

R3 gate:

- [x] Property and fixture tests cover supported answers, empty retrieval,
  contradictions, inference, stale citations, malformed JSON, unknown fields,
  source prompt injection, invented citations, oversized output, and failed
  repair.
- [x] Invalid output cannot cross the R3 validation boundary as a displayable
  answer.

## R4 — visible one-shot cited answers

Status: implemented on the Ollama-first route. This is the first-useful
checkpoint; the target-host tutorial demonstration remains an opt-in release
acceptance check.

Contract: the centre composer becomes a question interface while retaining
explicit evidence search.

- [x] Add distinct **Ask** and **Search** modes
- [x] Enable Ask only with an unlocked vault, attached model, question, and at
  least one retained source
- [x] Run retrieval, prompt assembly, inference, validation, and rendering as
  one visible task tree
- [x] Render summary, claim citations, warnings, and unresolved gaps
- [x] Open every citation in the retained-snapshot viewer
- [x] Show corrective errors for lock, detach, empty evidence, invalid output,
  timeout, and tunnel loss
- [x] Cancel inference and prevent late completion from updating the UI
- [x] Announce progress, completion, gaps, cancellation, and errors accessibly

R4 gate:

- Frontend unit/build tests cover Ask enablement; the native WebDriver shell
  gate covers both modes, locked-state enablement, keyboard submission, and
  task cancellation.
- Rust QA, retrieval, and transport fixtures cover successful validation, exact
  citation reopening, source prompt injection, tunnel loss, and cancellation.
- [x] A target-host demonstration answers from the tutorial pack with public
  internet access disabled; the full live acceptance is recorded under R6.

## R5 — encrypted persistent conversations

Status: implemented for local cited Q&A. The existing encrypted schema tables
are now used by a content-addressed conversation service; the filesystem
plaintext inspection passed in the R6 acceptance run.

Contract: completed questions and validated answers become immutable encrypted
messages. Invalid or cancelled generations never appear as completed answers.

- [x] Use the encrypted conversation/message schema already present in the
  vault baseline without a destructive migration
- [x] Store bodies as compressed content-addressed vault objects
- [x] Record role, order, model, task UUID, citations, and timestamp
- [x] Link edited replacements without mutating originals at the service layer
- [x] Restore ordered conversations after restart
- [x] Send only a bounded eight-message conversation window to the model
- [x] Quarantine partial and invalid generations by persisting assistant output
  only after R3 validation succeeds
- [x] Add explicit create, select, rename, and delete interactions

R5 gate:

- Lock, restart, ordering, replacement, and reference-count behavior is covered
  by the encrypted service and existing vault/object tests; cancellation and
  invalid-generation behavior is covered by the R3 transport/QA tests.
- [x] Filesystem inspection finds no question, answer, prompt, or citation text
  outside the mounted vault; the application-data and cache roots were scanned
  on 2026-09-15.

## R6 — Pinky Lite reliability and acceptance

Status: reliability implementation, deterministic offline acceptance, and the
configured target-host cited-conversation check are complete. Direct profile
inspection is clean; the operator-only SSH process-boundary review remains.

Contract: cited local Q&A remains trustworthy through restarts, failures,
unsupported questions, and public-internet disconnection.

- [x] Recover interrupted answer tasks as `failed_interrupted`
- [x] Retry only recoverable transport failures without duplicating messages
- [x] Detect model/tunnel disconnection and present a reconnect path
- [x] Retain bounded encrypted diagnostics
- [x] Provide keyboard, screen-reader, reduced-motion, and non-WebGL paths
- [x] Verify a clean desktop profile starts without a persisted model attachment
- [x] Verify the configured Ollama target host through the full offline scenario

The target-host check is executable as the ignored
`completes_project_alder_cited_conversation_without_web_fetches` test in
`crates/pinky-core/tests/live_ollama.rs`. On 2026-09-15 it passed all three
ignored checks against Ollama `qwen3.5:4b` through an explicit loopback
endpoint. The endpoint was temporarily forwarded to the configured local
Ollama host because this shell did not have the user's SSH credentials; no
public-web fetches are made by this scenario.

Acceptance scenario:

The deterministic offline core acceptance test first ingests the Project Alder
tutorial pack, retrieves retained evidence, validates a cited answer through a
local fixture provider, reopens its exact citation, and restores the encrypted
conversation after reopening the database. It also verifies that an empty
retrieval returns an explicit gap without an inference call and that a
pre-cancelled request cannot create an assistant message.

1. Disconnect public internet access.
2. Unlock a clean vault and ingest the Project Alder tutorial sources.
3. Attach Ollama directly or through the user's SSH tunnel.
4. Ask why IRIS417 was raised and what the operator should do.
5. Receive threshold, duration, event evidence, and runbook action claims whose
   citations reopen the exact retained passages.
6. Ask an unsupported question and receive an explicit unresolved gap.
7. Cancel generation and verify no completed partial answer or late update.
8. Restart Pinky and restore the validated conversation.

R6 gate:

- [x] All R0-R5 automated gates pass.
- [x] Deterministic offline Pinky Lite core acceptance passes.
- [x] Pinned clean-Debian install starts with a fresh profile and no model
  attachment.
- [x] The recorded target-host scenario passes without public internet.
- [x] The app process used an explicit loopback model endpoint, the temporary
  test forward and child processes were stopped, and no plaintext markers were
  found in the application-data or cache roots outside the mounted vault.
- [ ] Repeat the process-boundary check through the user's authenticated SSH
  tunnel on the target host; SSH credentials were unavailable in this shell.

## R7 — retrieval quality upgrade

Status: core contracts, encrypted desktop onboarding, and an opt-in
session-managed bridge are implemented; signed artifact and live acceptance
gates remain.

- [x] Add a bounded provider-neutral embedding contract and Ollama `/api/embed`
  transport with cancellation, response limits, model identity, and vector
  validation.
- [x] Validate a separate local Ollama embedding model and run a bounded smoke
  embedding before it can be used for indexing.
- [x] Add a batched retained-chunk embedding indexer that validates dimensions
  and upserts normalized vectors into the authenticated Qdrant client.
- [x] Add citation-preserving lexical/vector fusion with the existing RRF
  source-version cap.
- [x] Add cancellable core hybrid-search orchestration that embeds the query,
  queries Qdrant, and merges vector-only retained hits without losing citations.
- [x] Load current retained chunks from encrypted objects for embedding backfill
  without rereading original source files.
- [x] Add an opt-in desktop bridge that validates the embedding model, lazily
  starts one supervised Qdrant sidecar per application session, backfills and
  queries hybrid retrieval, reports retrieval phases, and retains lexical
  fallback when the bridge is not configured.
- [ ] Install and verify the signed embedding model and Qdrant executable.
- [ ] Run the retained-chunk backfill against a verified embedding model and
  Qdrant sidecar.
- [x] Add normal desktop onboarding, persistent encrypted configuration, and
  idle-managed sidecar shutdown.
- [x] Add a strict signed artifact manifest verifier with HTTPS, metadata,
  Ed25519, size, digest, redirect rejection, streamed download, and
  atomic-install checks.
- [x] Add a bounded deterministic reranker over the best 30 fused candidates,
  cap results at 12 citations, and cover relevance/bounding behavior with
  fixtures.
- [x] Add a warm reranker p95 smoke test; the target-host one-million-chunk
  retrieval benchmark remains a release acceptance gate.
- [ ] Install and verify signed embedding/Qdrant artifacts on the target host,
  run the retained-chunk backfill, and record warm p95 retrieval below 500 ms.

## R8 — prioritised image support

Status: complete for the retained local-image path. Image metadata ingestion,
bounded supervised OCR attachment, explicit retained-image viewing, worker
cancellation, and restart-safe searchable retention are implemented and
verified. PDF and Office extraction remain deliberately deferred.
PDF and Office extraction are explicitly deferred until this phase and its
acceptance gates pass.

Contract: approved image files remain encrypted, versioned, cancellable, and
inspectable without leaking pixels, OCR text, or metadata outside the mounted
vault.

Implemented and planned scope:

- [x] image metadata ingestion for PNG, JPEG, WebP, GIF, and TIFF;
- [x] encrypted retention of the original image and searchable retained JSON
  metadata with a versioned extraction method;
- [x] bounded OCR worker staging inside the mounted vault, with supervised
  process-group cancellation and an 8 MiB output bound;
- [x] OCR result attachment to source versions and searchable OCR citations;
- [x] exact retained-image viewing and source/version metadata;
- [x] cancellation of metadata/OCR work, discarded partial extraction, and
  exact reopening of retained image artifacts;
- [x] restart recovery keeps OCR chunks searchable and citations valid.

R8 gates cover vault-only pixel and OCR retention, deterministic image metadata,
isolated-worker limits, cancellation, and clean restart recovery. The dedicated
image cancellation test proves the supervised process is terminated and its
staging files are removed; the offline restart fixture reopens the vault and
searches the attached OCR citation again; the oversized-output fixture proves
the 8 MiB bound rejects and removes excessive OCR output.

## Parallel and deferred tracks

These do not delay R2-R6 unless the user changes priority:

- Supervised llama.cpp launch and model download onboarding
- PDF and Office extraction (deferred until after R8 image support)
- Image generation (deferred until a later creative-tools phase)
- Public-web research and freshness scheduling
- Claims, contradictions, and dossiers beyond answer warnings
- Autonomous workspace tools and generated applications
- Backup/restore, packaging, upgrades, accessibility acceptance, and full
  release acceptance

## Execution rule

Implement one phase at a time:

1. implement only that phase's contract;
2. run focused tests, full Rust/frontend regressions, strict Clippy, formatting,
   and diff checks;
3. record every passed, failed, skipped, and unavailable gate honestly;
4. update the acceptance ledger;
5. stop for review or a separately requested commit before the next phase.
