# Implementation status

This file is the acceptance ledger for the Pinky implementation specification.
`[x]` means verified here and `[ ]` means not implemented or not yet accepted.

## Stage 1 — foundation

- [x] Rust core workspace and native Tauri shell compilation
- [x] React production build and state-mapping unit test
- [x] Verified gocryptfs mount capability boundary
- [x] Content-addressed compressed object store
- [x] SQLCipher-only initial relational schema
- [x] Versioned task event contract
- [x] Cooperative in-process cancellation and hard abort deadline
- [x] Three-region task UI and task-state entity
- [x] Recovery-passphrase wrapping, domain-separated keys, and transactional onboarding tests
- [x] Live gocryptfs and Secret Service onboarding acceptance on the target host
- [x] Registered-vault discovery and Secret Service unlock after restart
- [x] Disconnected FUSE mount recovery and live-mount ownership protection
- [x] Durable event-log objects and startup interruption recovery
- [x] Supervised child-process SIGTERM/SIGKILL escalation
- [x] Stage-one Tauri end-to-end and clean-machine tests

## Stages 2–6

- [x] Approved-path local UTF-8 text ingestion, encrypted retention, versioning, and chunk metadata
- [x] Cross-source object deduplication and symlink-escape rejection
- [x] Stable-change local-file watching, automatic re-versioning, and missing-file state
- [ ] Image metadata, OCR, and isolated image extraction workers (prioritised R8)
- [ ] PDF and Office extractors (deferred until after R8)
- [x] Encrypted Tantivy lexical index and exact retained-version citation viewer
- [x] Authenticated loopback Qdrant supervision/client and reciprocal-rank fusion contract
- [x] Strict token-bearing loopback llama-server health client contract
- [x] Explicit no-key loopback Ollama attach, local-model validation, and live
  SSH-tunnel acceptance
- [x] Provider-neutral bounded and cancellable Ollama structured generation
  transport
- [x] Strict versioned evidence and cited-answer validation contract
- [x] Opt-in embedding model integration, end-to-end Qdrant vector indexing,
  and citation-preserving hybrid retrieval
- [x] Signed artifact manifest verification and atomic, checksum-verified
  installation contract, including bounded streamed downloads with redirects
  rejected
- [x] Bounded hybrid reranking over at most 30 fused candidates, returning at
  most 12 citations with deterministic relevance fixtures
- [x] One-shot cited Q&A over retained lexical evidence with explicit Ask/Search
  modes, cancellable task events, validated claims, warnings, gaps, and exact
  citation reopening
- [ ] Signed model/Qdrant artifact onboarding, target-host retained-chunk
  backfill, default hybrid cited chat, claims, dossiers, and the one-million
  chunk warm p95 retrieval gate
- [x] Encrypted persistent conversations with immutable ordered messages,
  bounded history, validated-answer persistence, and create/select/rename/delete
  desktop interactions
- [ ] Safe web fetch, SearXNG, Chromium, research, and refresh scheduling
- [ ] Workspace snapshots, permissions, Podman tools, and app generation
- [ ] Image generation (deferred creative-tools phase)
- [ ] Accessibility acceptance, backup, packaging, and upgrades
- [x] Pinky Lite reliability path: interrupted-task recovery, bounded retry for
  recoverable model transport failures, reconnect guidance, and validated
  cancellation/error handling

## Verification on this checkout

Frontend unit tests and production compilation pass with Node.js. The core Rust
tests pass using a temporary stable toolchain and vendored SQLCipher/OpenSSL.
Full Tauri-native compilation passes on the target host. The dependency-free
WebDriver protocol harness uses an isolated XDG profile and verifies the
compiled WebKit application shell, a real Tauri path command, task events, and
cancellation. This acceptance run exposed and fixed a synchronous Tauri command
trying to spawn work without a Tokio runtime.

Release Debian and AppImage bundles build successfully. Bundle inspection
validates the Debian architecture and installed files, extracts the AppImage
without FUSE, verifies its runtime layout, and reports SHA-256 artifact
metadata. The Debian package also passes a pinned Debian 13 clean-machine test:
it installs with its declared dependencies, has no unresolved shared libraries,
and remains running as an unprivileged user in a fresh Xvfb and D-Bus session.

The first Stage 2 slice accepts an explicit approved directory and local file.
It canonicalizes both paths, opens the source without following a swapped final
symlink, verifies the opened descriptor still resolves inside the approved
root, and refuses non-regular files. Original bytes are streamed into encrypted
content-addressed storage before extraction. UTF-8 text, Markdown, logs, source
code, JSON, YAML, XML, HTML, and CSV are normalized for line endings, retained,
and chunked at 500 approximate tokens with 75-token overlap. Metadata changes
atomically create a new source version and move the current-version pointer;
unsupported binaries are still archived and visibly marked unsupported.

The desktop starts a supervised local watcher after vault creation or unlock.
Each retained version records device, inode, size, and nanosecond modification
time. Changed files must hold the same size and modification time through two
checks at least 500 ms apart and a 750 ms debounce window before re-ingestion.
Failed or cancelled replacements leave the previous current version active and
retry after a bounded cooldown. Deletions mark the source `missing` while
retaining its current version and archived objects; reappearance schedules a
new version after the same stability checks.

Extracted chunks are indexed by Tantivy inside the mounted encrypted vault.
Ingestion commits an index version before moving the relational current-version
pointer, while search filters out superseded versions by default. A count
mismatch rebuilds the generated lexical index from retained chunk objects, so
sources ingested by earlier builds become searchable without reading the
original local file again. The desktop composer performs lexical retrieval and
opens exact `pinky://source/.../version/...#chunk-...` citations with retained
passage text, source provenance, coordinates, and retrieval time. Qdrant,
reranking, answer generation, and cited conversational responses remain later
slices.

The vector boundary prepares a supervised Qdrant process on random
loopback-only HTTP and gRPC ports with a new 256-bit API key on every managed
launch. Explicit attachment also supports Ollama's unauthenticated local API on
an exact IPv4-loopback origin; remote Ollama and Ollama cloud endpoints remain
out of scope.
Its storage and snapshots are constrained to checked directories inside the
mounted vault, and dropping the sidecar aborts the same process-group supervisor
used by cancellable workers. The client creates a cosine collection with HNSW
and scalar int8 quantization on disk, normalizes vectors, upserts UUID points,
and queries with authenticated requests that bypass ambient proxies. Reciprocal
rank fusion uses `k = 60`, deduplicates chunk UUIDs, and caps results at three
chunks per source version. This is available through the documented opt-in
bridge; signed artifact onboarding and default user-facing configuration remain
gated before it can become the normal retrieval path.

R7 groundwork now includes a bounded provider-neutral embedding contract, an
Ollama `/api/embed` transport with cancellation and vector validation, and a
separate embedding-model probe that rejects completion-only, non-GGUF, or
remote models before running a bounded smoke embedding. A batched retained-
chunk indexer can upsert into the authenticated Qdrant client. The retrieval
service can now load only current retained chunks from encrypted objects for
backfill, without rereading original files. A cancellable core hybrid-search
method now embeds a query, retrieves lexical and vector candidates, and
preserves exact citations while applying the existing RRF source-version cap.
The desktop bridge now validates the embedding model, starts one supervised
Qdrant sidecar lazily per application session, replaces an unhealthy sidecar,
reports retrieval phases, reuses the encrypted vector index for subsequent
searches, and shuts the sidecar down after five idle minutes. The runtime panel
can verify and store the Qdrant executable and embedding settings in the
encrypted SQLCipher database; without a saved configuration or all three
development fallback values it retains the lexical path. Signed artifact
onboarding and live acceptance remain gated.

The llama.cpp compatibility boundary accepts only an explicit
`http://127.0.0.1:<port>` llama-server origin, requires a 256-bit hexadecimal
bearer token, bypasses ambient proxies, redacts the token from diagnostics, and
validates a bounded, cancellable health request. A deterministic loopback test
verifies the request path and authorization header. The upstream health route
is public, so authentication is also proven against a protected endpoint.

The desktop now exposes the attach portion of Q1B after vault unlock. It accepts
an ephemeral endpoint and token, checks the public health route, proves the
token against the protected `/props` route, validates a non-empty model path,
at least one slot, and at least 2,048 context tokens, then shows the attached
model and context size. Detaching or losing the verified vault drops the client
and zeroizes its in-memory token. Supervised executable launch, shutdown
acceptance, and generation through this compatibility provider remain deferred;
the Ollama-first delivery route is unaffected.

The Ollama-first runtime path is accepted independently of managed
`llama-server`. Pinky accepts no-key Ollama only through an exact IPv4-loopback
origin, rejects remote/cloud model metadata, and requires a local GGUF
completion model with at least 2,048 context tokens. Native desktop state tests
cover the vault and attachment lifecycle. The opt-in target-host test passed on
2026-09-14 through the user's SSH tunnel at `127.0.0.1:11435`, probing Ollama
0.33.2 and `qwen3.5:9b` with 262,144 advertised context tokens. No generation
request was made during this phase.

The R2 generation transport posts non-streaming `/api/chat` requests with an
explicit JSON Schema, zero temperature, disabled thinking, bounded output
tokens, and a five-minute keep-alive. It bounds both serialized request bytes
and response bytes even when `Content-Length` is absent, distinguishes
transport failures, and discards late responses after cancellation. Response
model identity and remote/cloud metadata are validated before returning
untrusted content. A fixed non-sensitive structured-output test passed through
the target-host tunnel against `qwen3.5:9b`; generated text is not yet displayed
or persisted.

R3 adds the non-displayable trust boundary between retrieval and the future
question UI. It selects no more than 12 current-version passages and 8,000
approximate context tokens, deduplicates chunks, caps each source version at
three passages, and regenerates citation identifiers from trusted source,
version, and chunk coordinates. Evidence is serialized inside a task-specific
untrusted-data delimiter. Model output must match a bounded, versioned JSON
contract; every summary and claim citation must be one of the supplied IDs,
inferences must be explicit, and malformed or unsupported output receives at
most one bounded repair attempt. Empty retrieval returns an explicit uncited
gap without calling the model.

R4 wires that boundary into the desktop. Ask and Search are explicit modes;
Ask stays disabled until the vault, a retained source, and an attached local
model are all available. A single cancellable task reports retrieval,
evidence assembly, inference, and validation progress. Only validated answers
render, with clickable summary and claim citations, warning/gap panels, and
the existing exact retained-snapshot viewer. Cancellation and a per-request
generation guard prevent late model results from appearing after a stop or
failure. The target-host tutorial demonstration remains an opt-in acceptance
check; persistent conversations are R5.

R5 adds the encrypted conversation service over the existing SQLCipher schema.
Conversation and message metadata remain in the vault database; user and
assistant bodies are compressed, content-addressed vault objects whose
reference counts are updated transactionally. Messages are immutable and
ordered, preserve model/citation/task metadata, and support replacement links.
The desktop restores conversations after unlock, limits model history to the
last eight messages, persists an assistant message only after R3 validation,
and exposes encrypted create/select/rename/delete controls. Invalid or
cancelled inference never creates a completed assistant message. The direct
filesystem plaintext inspection passed in the R6 acceptance run.

R6 completes the automated Pinky Lite reliability path. Existing encrypted task
journals recover interrupted work as `failed_interrupted`; cited answers retry
once only for recoverable transport failures, without creating duplicate
assistant messages. Transport failures expose a reconnect action, while task
events and bounded provider error bodies remain encrypted diagnostics. The
desktop retains keyboard navigation, live announcements, reduced-motion
support, and a non-WebGL text fallback. The deterministic offline acceptance
test ingests the Project Alder tutorial pack, validates a fixture-backed cited
answer, reopens its exact citation, exercises an explicit evidence gap and
pre-cancelled request, then restores the encrypted conversation after restart.
The pinned clean-Debian install and fresh-profile E2E check pass without a
persisted model attachment. On 2026-09-15 the full Project Alder target-host
scenario passed all three live Ollama checks through an explicit loopback
endpoint, and the direct plaintext scanner found no markers in the
application-data or cache roots outside the mounted vault. The live check used
a temporary local forward because SSH credentials were not available in this
shell; repeat the SSH-specific process-boundary review on the target host
before release.

The onboarding transaction is covered through a platform test double, including
authenticated recovery, path and symlink boundaries, SQLCipher creation, and
failure rollback. The opt-in target-host acceptance test also passed with real
gocryptfs and Secret Service: it created and mounted a temporary vault, retained
and verified an encrypted object, unmounted, reopened the registered vault from
its stored root key, verified the same object, and removed the temporary secret
and vault.

Registered-vault recovery distinguishes a responsive mount owned by another
Pinky process from a disconnected FUSE endpoint left by forced termination.
Responsive mounts are never displaced; stale endpoints are detached before a
freshly authenticated gocryptfs process starts. The target host's existing
registered vault was verified through this path from Secret Service lookup
through SQLCipher open and clean unmount.

Restart tests cover bounded, owner-only registration metadata, strict schema and
recovery-identity validation, Secret Service key retrieval, remounting, and
SQLCipher reopening. Corrupt registration is reported and is never overwritten
by a new setup.

Task-journal tests cover full monotonic event histories in compressed,
content-addressed vault objects; SQLCipher object references and reference-count
replacement; restart conversion from `running` to durable
`failed_interrupted`; and refusal to recover a checksum-corrupt log. The
desktop attaches this journal immediately after vault creation or unlock and
reports later persistence failures in runtime status.

Process-supervision tests cover normal exit status, task-bound cancellation,
whole-process-group `SIGTERM`, escalation to `SIGKILL` when termination is
ignored, descendant cleanup on cancellation and future abort, and preservation
of cancellation failures. A
post-`SIGKILL` reap timeout reports possible kernel-level uninterruptible sleep
instead of emitting a successful cancellation state.

## Architecture boundaries

The Tauri process owns lifecycle. Commands and events are the sole UI/core
transport. Sidecars will bind only to random loopback ports and receive a fresh
256-bit bearer token per launch. No model or public fetch runtime may access the
vault directly; source bytes enter through the ingestion boundary and all model
output is treated as untrusted until validated.
