# Cited local Q&A development plan

Status: active, cost-first delivery contract.

This plan starts from Pinky's encrypted source search and attached Ollama
runtime. Its first objective is narrow but useful: ask a question about approved
local text sources and receive a local answer whose citations reopen the exact
retained passages.

Model downloads, supervised `llama-server`, embeddings, Qdrant, PDF extraction,
and web research are not prerequisites for the first usable release. A checked
item does not complete a phase until every gate for that phase passes.

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

Status: next implementation phase.

Contract: Pinky submits a bounded structured-output request to the attached
Ollama model and returns an untrusted response to the validator. Generated text
is not displayed or persisted yet.

Ollama's documented [`/api/chat`](https://docs.ollama.com/api/chat) request
accepts a JSON Schema in `format`; Pinky still validates the returned content
itself because runtime schema enforcement does not make model output trusted.

- [ ] Add a provider-neutral inference trait
- [ ] Implement Ollama `POST /api/chat` with `stream: false` for the initial
  structured-output slice
- [ ] Supply a JSON Schema in `format`, deterministic generation options, and
  an explicit bounded `keep_alive`
- [ ] Verify the response model and reject remote/cloud response metadata
- [ ] Bound request size, response size, connection time, inference time, and
  retained error bodies
- [ ] Support cooperative cancellation and discard late responses
- [ ] Return timing and token counts without logging request or response bodies
  outside the vault
- [ ] Distinguish unavailable server, missing model, timeout, cancellation,
  malformed response, and remote response errors

R2 gate:

- Fake-Ollama tests cover exact request shape, schema, absence of authorization
  and proxy use, success, defined errors, cancellation, late responses, and
  size limits.
- A target-host smoke test submits a non-sensitive fixed prompt and validates a
  schema-conforming response.

## R3 — evidence and answer contract

Status: specified, not implemented.

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
- `claims`: ordered `statement`, `support`, and `citations` objects;
- `warnings`: stale, disputed, or single-source qualifications; and
- `unresolved_gaps`: matters the supplied evidence could not answer.

`support` is `direct`, `inference`, or `disputed`. Direct and disputed
claims require supplied citations. Inferences require cited premises and
explicit inference wording. Unknown fields, empty or duplicate claims, invented
citations, and unbounded strings or arrays are rejected.

### Retrieval and prompt assembly

- [ ] Retrieve only current versions through the existing lexical index
- [ ] Deduplicate chunks and cap each source version at three chunks
- [ ] Select at most 12 chunks and 8,000 approximate context tokens
- [ ] Preserve contradictory passages
- [ ] Delimit evidence as untrusted quoted material
- [ ] Supply only Pinky-generated citation IDs
- [ ] Validate output independently of Ollama's schema enforcement
- [ ] Permit at most one bounded repair request
- [ ] Return an evidence gap without inference when retrieval is empty

R3 gate:

- Property and fixture tests cover supported answers, empty retrieval,
  contradictions, inference, stale citations, malformed JSON, unknown fields,
  source prompt injection, invented citations, oversized output, and failed
  repair.
- Invalid output must never become a displayable answer.

## R4 — visible one-shot cited answers

Status: not implemented. Completion is the first-useful checkpoint.

Contract: the centre composer becomes a question interface while retaining
explicit evidence search.

- [ ] Add distinct **Ask** and **Search** modes
- [ ] Enable Ask only with an unlocked vault, attached model, question, and at
  least one retained source
- [ ] Run retrieval, prompt assembly, inference, validation, and rendering as
  one visible task tree
- [ ] Render summary, claim citations, warnings, and unresolved gaps
- [ ] Open every citation in the retained-snapshot viewer
- [ ] Show corrective errors for lock, detach, empty evidence, invalid output,
  timeout, and tunnel loss
- [ ] Cancel inference and prevent late completion from updating the UI
- [ ] Announce progress, completion, gaps, cancellation, and errors accessibly

R4 gate:

- Frontend tests cover modes, enablement, rendering, errors, and keyboard use.
- Native tests cover successful Q&A, exact citation reopening, source prompt
  injection, tunnel loss, and cancellation.
- A target-host demonstration answers from the tutorial pack with public
  internet access disabled.

## R5 — encrypted persistent conversations

Status: not implemented.

Contract: completed questions and validated answers become immutable encrypted
messages. Invalid or cancelled generations never appear as completed answers.

- [ ] Add conversation and immutable message migrations
- [ ] Store bodies as compressed content-addressed vault objects
- [ ] Record role, order, model, task UUID, citations, and timestamp
- [ ] Link edited replacements without mutating originals
- [ ] Restore ordered conversations after restart
- [ ] Send only a bounded conversation window to the model
- [ ] Quarantine partial and invalid generations
- [ ] Add explicit create, select, rename, and delete interactions

R5 gate:

- Lock, restart, ordering, replacement, corruption, cancellation, and
  reference-count tests pass.
- Filesystem inspection finds no question, answer, prompt, or citation text
  outside the mounted vault.

## R6 — Pinky Lite reliability and acceptance

Status: not implemented. Completion is the Pinky Lite checkpoint.

Contract: cited local Q&A remains trustworthy through restarts, failures,
unsupported questions, and public-internet disconnection.

- [ ] Recover interrupted answer tasks as `failed_interrupted`
- [ ] Retry only recoverable transport failures without duplicating messages
- [ ] Detect model/tunnel disconnection and present a reconnect path
- [ ] Retain bounded encrypted diagnostics
- [ ] Pass keyboard, screen-reader, reduced-motion, and non-WebGL checks
- [ ] Cover the Ollama configuration in packaging and clean-account tests

Acceptance scenario:

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

- All R0-R5 automated gates pass.
- The recorded target-host scenario passes without public internet.
- No direct LAN/external model address, cloud-backed model, orphaned request, or
  plaintext conversation artifact is found.

## R7 — retrieval quality upgrade

Status: deferred until Pinky Lite.

- Install and verify the signed embedding model and Qdrant executable.
- Backfill embeddings from retained text without rereading originals.
- Combine lexical and vector results through reciprocal-rank fusion.
- Add reranking, relevance fixtures, and warm p95 performance acceptance.

## Parallel and deferred tracks

These do not delay R2-R6 unless the user changes priority:

- Supervised llama.cpp launch and model download onboarding
- PDF, Office, image metadata, and OCR extraction
- Public-web research and freshness scheduling
- Claims, contradictions, and dossiers beyond answer warnings
- Autonomous workspace tools and generated applications
- Image generation, backup/restore, and full release acceptance

## Execution rule

Implement one phase at a time:

1. implement only that phase's contract;
2. run focused tests, full Rust/frontend regressions, strict Clippy, formatting,
   and diff checks;
3. record every passed, failed, skipped, and unavailable gate honestly;
4. update the acceptance ledger;
5. stop for review or a separately requested commit before the next phase.
