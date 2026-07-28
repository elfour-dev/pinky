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
- [ ] PDF, office, image/OCR, and isolated-worker extractors
- [ ] Tantivy, Qdrant, and citation viewer
- [ ] Model onboarding, hybrid retrieval, cited chat, claims, and dossiers
- [ ] Safe web fetch, SearXNG, Chromium, research, and refresh scheduling
- [ ] Workspace snapshots, permissions, Podman tools, and app generation
- [ ] Image generation, accessibility acceptance, backup, packaging, upgrades

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
