# Pinky development roadmap

Recorded: 2026-09-21

This is the implementation-facing roadmap for the remaining Pinky work. The
acceptance ledger in [`IMPLEMENTATION_STATUS.md`](IMPLEMENTATION_STATUS.md) is
the source of truth for individual checks; this document explains the order,
dependencies, and definition of done for the remaining product milestones.

## Active phase gate

Only the incomplete R7 and R9 workstreams are active. R10 and every later
phase are blocked until R7 and R9 have passed their remaining target-host
acceptance checks and the acceptance ledger has been updated. New feature work
must not be started in a later phase while either active gate is open.

## Where the project is now

| Area | Status | Meaning |
| --- | --- | --- |
| Encrypted vault, retention, local text ingestion, watching, citations | Complete | The retained local-text foundation is usable. |
| Local Ollama cited conversations | Complete | Pinky Lite works offline with a configured local model. |
| Reliability, cancellation, privacy, and clean-install checks | Complete for Pinky Lite | The remaining SSH process-boundary review is an acceptance task. |
| Image metadata, retained images, OCR, deletion, and diagnostics | Complete for the retained-image scope | Image generation is not included in this milestone. |
| Hybrid retrieval | Implemented but release-gated | Local warm retrieval and reranking benchmarks pass; signed artifact onboarding and target-host backfill remain. |
| PDF extraction | Implemented locally but release-gated | Embedded-text extraction, image-only rendering/OCR, restart recovery, and the fixture suite pass; interrupted target-host acceptance remains. |
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
active target-host acceptance gate, while Office extraction remains the
final-stage compatibility phase.

## Ordered remaining roadmap

### R7 close — hybrid retrieval and model onboarding

**Status:** implementation and local acceptance are complete; target-host
acceptance remains.

**Deliverables**

- Install and verify signed embedding and Qdrant artifacts.
- Backfill current retained chunks without rereading original sources; the
  implementation now resumes at the first uncommitted batch and isolates
  collections by embedding identity.
- Make hybrid retrieval an explicit, well-diagnosed user configuration rather
  than a development-only path.
- Complete managed `llama-server` launch only if the compatibility provider is
  still required; Ollama remains the first supported route.

**Exit gate:** target-host onboarding succeeds, the backfill is restart-safe,
citations remain exact, and warm retrieval over one million chunks is below the
500 ms p95 target. The local one-million-chunk and reranking warm-p95 checks
passed on 2026-09-21. Signed artifact installation, verified target-host
backfill, the target-host benchmark record, and the authenticated SSH
process-boundary check must still be completed before release claims are made.

### R9 — PDF extraction

**Status:** implementation and local fixture acceptance are complete; only
interrupted target-host acceptance remains.

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
the target-host interrupted-process demonstration is still required.

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
passing automated or documented target-host demonstration, with no hidden,
skipped, or unrecorded failures.

## Dependencies and parallelism

The critical path is:

```text
R7 close -> R9 PDF -> R10 evidence -> R11 web research
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
record any unavailable target-host gate honestly. A phase is not complete merely
because its UI exists or its code compiles.

## Immediate next action

1. Close R7: install and verify signed artifacts, run the target-host
   retained-chunk backfill, record the warm one-million-chunk result, and
   complete the authenticated SSH process-boundary review.
2. Close R9: run the interrupted-process PDF acceptance on the target host and
   confirm no orphan workers or partial artifacts remain.
3. Re-run the full regression and acceptance gates. Do not start R10 until
   both R7 and R9 are marked complete in the acceptance ledger.

Office extraction should not be started until the final-stage entry criteria in
R14 are met.
