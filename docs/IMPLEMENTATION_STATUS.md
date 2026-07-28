# Implementation status

This file is the acceptance ledger for the Pinky implementation specification.
`[x]` means verified here and `[ ]` means not implemented or not yet accepted.

## Stage 1 — foundation

- [x] Rust core workspace (native shell compilation still requires host WebKit/GTK development packages)
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
- [x] Durable event-log objects and startup interruption recovery
- [x] Supervised child-process SIGTERM/SIGKILL escalation
- [ ] Stage-one Tauri end-to-end and clean-machine tests

## Stages 2–6

- [ ] Local extraction, watching, Tantivy, Qdrant, and citation viewer
- [ ] Model onboarding, hybrid retrieval, cited chat, claims, and dossiers
- [ ] Safe web fetch, SearXNG, Chromium, research, and refresh scheduling
- [ ] Workspace snapshots, permissions, Podman tools, and app generation
- [ ] Image generation, accessibility acceptance, backup, packaging, upgrades

## Verification on this checkout

Frontend unit tests and production compilation pass with Node.js. The core Rust
tests pass using a temporary stable toolchain and vendored SQLCipher/OpenSSL.
Full Tauri-native compilation reached the native dependency build and stopped
because this host lacks `pkg-config` and the D-Bus development package; the
later WebKit/GTK development packages may also be required. Installing host
packages was outside this implementation run, so native end-to-end testing
remains an unchecked stage-one gate.

The onboarding transaction is covered through a platform test double, including
authenticated recovery, path and symlink boundaries, SQLCipher creation, and
failure rollback. The opt-in target-host acceptance test also passed with real
gocryptfs and Secret Service: it created and mounted a temporary vault, retained
and verified an encrypted object, unmounted, reopened the registered vault from
its stored root key, verified the same object, and removed the temporary secret
and vault.

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
