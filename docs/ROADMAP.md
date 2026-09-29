# Pinky development roadmap

Recorded: 2026-09-21

This is the implementation-facing roadmap for the remaining Pinky work. The
acceptance ledger in [`IMPLEMENTATION_STATUS.md`](IMPLEMENTATION_STATUS.md) is
the source of truth for individual checks; this document explains the order,
dependencies, and definition of done for the remaining product milestones.

## Distribution posture

Pinky is a personal application that may be shared with a small trusted circle;
it is not planned as a public release. Its owner controls the artifact-signing
private key and builds that contain the corresponding public key. Public key
hosting, a public package channel, and third-party release operations are out
of scope. This does not relax integrity requirements: artifacts still require
an owner-approved bundled public key, a valid signature, HTTPS, and matching
size and digest before installation.

## Active phase gate

Only the incomplete R9 workstream is active. R10 and every later phase are
blocked until its remaining private-host acceptance check passes and the
acceptance ledger has been updated. New feature work must not be started in a
later phase while that gate is open.

## Where the project is now

| Area | Status | Meaning |
| --- | --- | --- |
| Encrypted vault, retention, local text ingestion, watching, citations | Complete | The retained local-text foundation is usable. |
| Local Ollama cited conversations | Complete | Pinky Lite works offline with a configured local model. |
| Reliability, cancellation, privacy, and clean-install checks | Complete for Pinky Lite | The remaining SSH process-boundary review is an acceptance task. |
| Image metadata, retained images, OCR, deletion, and diagnostics | Complete for the retained-image scope | Image generation is not included in this milestone. |
| Hybrid retrieval | Complete | Owner-signed Qdrant onboarding, retained-chunk backfill, source-grounded cited Ask/Search, and private-host performance checks passed on 2026-09-29. |
| PDF extraction | Implemented locally but owner-gated | Embedded-text extraction, image-only rendering/OCR, restart recovery, and the fixture suite pass; interrupted private-host acceptance remains. |
| Office extraction | Not started | DOCX, XLSX, PPTX, and ODT are deliberately final-stage work. |

The current application can answer from retained local sources when a local
Ollama model is attached. It does not yet perform public-web research, create
workspace projects, generate images, or provide the complete version-one
acceptance surface.

## Completed milestone: R8 — prioritised image support

R8 is complete for the retained local-image scope. It delivered:

- encrypted, versioned ingestion for PNG, JPEG, WebP, GIF, and TIFF;
- searchable image metadata and explicit retained-image viewing;
- bounded supervised OCR with automatic page-segmentation/confidence
  attempts;
- searchable OCR citations, individual and bulk detection deletion;
- cancellation, restart recovery, task diagnostics, and partial-artifact
  cleanup.

Image generation is intentionally not part of R8; it remains the later R13
creative-tools phase. PDF extraction is implemented locally and remains an
active private-host acceptance gate, while Office extraction remains the
final-stage compatibility phase.

## Ordered remaining roadmap

### R7 — hybrid retrieval and model onboarding

**Status:** complete. On 2026-09-29 the owner verified signed Qdrant
installation through the bundled development public key, the local
`nomic-embed-text:latest` embedding model, retained-chunk backfill, and
source-grounded cited Ask/Search. The one-million-chunk warm retrieval and
warm reranking acceptance tests also passed on the private development host;
both enforce the required p95 below 500 ms.

**Deliverables**

- Have the owner sign and install approved Qdrant artifacts, using a public
  key bundled into the private build. Record the local Ollama embedding
  model's validated digest.
- Backfill current retained chunks without rereading original sources; the
  implementation now resumes at the first uncommitted batch and isolates
  collections by embedding identity.
- Make hybrid retrieval an explicit, well-diagnosed user configuration rather
  than a development-only path.
- Complete managed `llama-server` launch only if the compatibility provider is
  still required; Ollama remains the first supported route.

**Exit gate:** passed. Direct loopback Ollama is in use, so the SSH
process-boundary review is not applicable.

### R9 — PDF extraction

**Status:** implementation and local fixture acceptance are complete; only
interrupted private-host acceptance remains.

**Deliverables**

- Retain the original PDF byte-for-byte in the encrypted object store and run
  embedded-text extraction through a bounded Poppler worker.
- Preserve page-aware chunks and exact retained-version citations for embedded
  text.
- Detect embedded text and image-only pages from retained Poppler output.
- Extract page-aware text in bounded supervised workers, including bounded
  `pdftoppm` rendering for blank pages.
- Reuse the existing OCR path for image-only pages when local Tesseract is
  available, recording the page in its citation coordinates.
- Reopen exact PDF-version citations with page and coordinate information.

**Exit gate:** text, image-only, mixed, malformed, encrypted, oversized, and
cancelled PDF fixtures pass extraction, indexing, citation, restart, and
resource-limit tests. Failed replacement indexing leaves the previous version
searchable. The complete local fixture suite passed on 2026-09-21 (12 tests);
the private-host interrupted-process demonstration is still required.

### R10 — claims, dossiers, freshness, and evidence quality

**Status:** not started.

**Deliverables**

- Extract source-grounded claims, entities, aliases, and relationships.
- Add topic dossiers with coverage, authority, independence, freshness, and
  unresolved-question scores.
- Preserve supporting and contradicting evidence instead of hiding conflicts.
- Classify source freshness and schedule bounded refreshes for stale sources.
- Add warnings for disputed, stale, single-source, and inferred statements.

**Exit gate:** deterministic fixtures prove claim status transitions,
contradiction preservation, dossier consolidation, coverage calculation, and
freshness scheduling without allowing model output to create unsupported facts.

### R11 — public-web research and safe refresh

**Status:** safety-boundary slice implemented; online orchestration remains a
separate opt-in capability and the phase is not complete.

**Deliverables**

- Supervise local SearXNG and Chromium services when needed.
- Generate at most five searches, archive result pages/snippets, and fetch only
  public permitted origins.
- Enforce robots rules, cache and retry headers, domain blocks, SSRF/LAN
  protection, crawl depth, origin, byte, time, and concurrency budgets.
- Trigger research for inadequate, stale, contradictory, or explicitly
  current/verified questions, then rerun retrieval.
- Ask a blocking question when a budget or permission must be expanded.

**Exit gate:** research fixtures demonstrate safe rejection of private/LAN/
loopback targets, prompt-injection resistance, budget exhaustion, cancellation,
deduplication, source-level citations, and a clearly marked “researched now”
answer.

### R12 — approved workspaces and generated applications

**Status:** not started.

**Deliverables**

- Add approved workspace roots, canonical path checks, and per-task permission
  scopes.
- Create content-addressed recovery snapshots before changing existing files.
- Run installs, builds, tests, and linters only in rootless Podman containers
  with the specified CPU, memory, process, timeout, filesystem, and network
  limits.
- Generate projects with source, lockfiles, tests, instructions, logs, and an
  artifact manifest.
- Show diffs and support per-task rollback without automatic Git operations.

**Exit gate:** workspace, symlink, secret, privilege, network, container escape,
resource-exhaustion, cancellation, rollback, and artifact-completeness tests
all pass.

### R13 — image generation and creative tools

**Status:** deliberately deferred; not required for the next usable release.

**Deliverables**

- Supervise the approved local diffusion runtime only when requested.
- Support text-to-image and image-to-image with negative prompt, seed, size,
  steps, guidance, and one-to-four output controls.
- Record a JSON sidecar for every completed PNG and quarantine partial output.
- Enforce the single heavy-model memory budget and step-level cancellation.

**Exit gate:** CPU/fallback and Vulkan paths, deterministic metadata, model
  checksum recording, cancellation, memory pressure, and no-partial-result
  tests pass.

### R14 — final document compatibility: Office extraction

**Status:** final-stage feature, after the core assistant, research, workspace,
generation, and recovery surfaces have stabilised.

**Deliverables**

- Implement separate bounded extractors for DOCX, XLSX, PPTX, and ODT.
- Preserve headings, paragraphs, tables, sheets, formulas, slides, and visible
  text as applicable.
- Never execute macros, embedded programs, or active content.
- Preserve structural coordinates in citations and retain original archives.

**Exit gate:** each format has independent fixtures for valid, corrupt,
decompression-bomb, oversized, and cancellation cases; exact structural
citations reopen the retained version and clean-machine tests pass.

### R15 — release hardening and version-one acceptance

**Status:** not started.

**Deliverables**

- Encrypted backup and restore with checksum verification and consistent
  database snapshots.
- Two-version schema/index migrations and automatic pre-migration backups.
- Seven-day encrypted trash, confirmed permanent purge, and explicit backup
  retention warnings.
- Final packaging, clean-machine installation, accessibility, reduced-motion,
  keyboard, screen-reader, non-WebGL, and upgrade tests.
- Complete task-tree observability and cancellation checks for every worker.
- Resolve the final product name and perform one controlled user-facing rename;
  Pinky remains the repository/package working title until then.

**Exit gate:** every version-one acceptance scenario in the specification has a
passing automated or documented private-host demonstration, with no hidden,
skipped, or unrecorded failures.

## Dependencies and parallelism

The critical path is:

```text
R9 PDF -> R10 evidence -> R11 web research
                              \-> R12 workspaces
                              \-> R13 image generation
R10/R11/R12/R13 -> R14 Office -> R15 release hardening
```

R12 and R13 can proceed independently once the shared task, permissions, and
artifact contracts are stable. R14 must remain last among document extractors:
Office support must not delay the PDF milestone or core research/workspace
work. Backup, migration, and packaging work should be exercised incrementally,
but their final acceptance belongs to R15.

## Definition of done for every phase

Each phase must implement only its stated contract, add focused unit/property
and fixture tests, run the relevant full Rust and frontend regressions, run
formatting, Clippy, and `git diff --check`, update the acceptance ledger, and
record any unavailable private-host gate honestly. A phase is not complete merely
because its UI exists or its code compiles.

## Immediate next action

1. Close R9: run the interrupted-process PDF acceptance on the designated
   private host and
   confirm no orphan workers or partial artifacts remain.
2. Re-run the full regression and acceptance gates. Do not start R10 until R9
   is marked complete in the acceptance ledger.

Office extraction should not be started until the final-stage entry criteria in
R14 are met.
