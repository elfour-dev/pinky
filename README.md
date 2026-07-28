# Pinky

Pinky is a private, source-grounded Linux desktop assistant. This repository is
being delivered in six acceptance-gated stages; it is not yet a version-one
release.

## Current milestone

The executable stage-one foundation currently includes:

- a Tauri 2 / React / TypeScript desktop shell with no externally reachable HTTP API;
- a Rust core whose persistence APIs require a verified gocryptfs mount;
- streaming SHA-256 content addressing, MIME-aware Zstandard compression,
  atomic installation, deduplication, and verified reads;
- a SQLCipher-only metadata connection and versioned initial schema;
- transactional vault creation using gocryptfs and Linux Secret Service, with a
  passphrase-protected recovery envelope and domain-separated storage keys;
- monotonic task events, task trees, pause checkpoints, cooperative cancellation,
  and a twelve-second final abort deadline for in-process workers;
- task-bound process-group supervision that sends `SIGTERM` after two seconds,
  `SIGKILL` after ten seconds, and prevents descendants from becoming orphans;
- compressed task-event histories retained as referenced vault objects, with
  interrupted work durably changed to `failed_interrupted` after restart;
- a three-region UI, live xterm event log, task controls, and a Three.js entity
  driven by task state, including reduced motion and a non-WebGL fallback.

Vault creation and restart-time unlocking are ready for native acceptance on a
host with the required Linux packages, but this checkout's host does not
currently provide them. Ingestion, retrieval, models, research, generated-code
containers, image generation, backup/restore, and release packaging remain
gated work. The UI intentionally refuses conversations and retained-data
operations while the vault is unavailable.

## Development

Prerequisites for building are Node.js 20+, Rust stable, the Tauri 2 Linux system
dependencies, SQLCipher build dependencies, and a C toolchain.

```bash
cd apps/desktop
npm install
npm test
npm run build
cd ../..
cargo test --workspace
```

Run the desktop application with:

```bash
cd apps/desktop
npm run tauri dev
```

Production onboarding will additionally require gocryptfs, Linux Secret Service,
rootless Podman, and Vulkan. Pinky will never treat a normal directory as an
encrypted vault.

Vault creation generates a random root key, derives independent gocryptfs and
SQLCipher keys, stores the root key through Secret Service, and writes an
Argon2id/XChaCha20-Poly1305 recovery envelope. The recovery passphrase is never
stored. Both selected directories must be absolute, canonical, and empty.
Pinky stores a separate owner-only registration containing only the vault UUID
and canonical paths. On later launches it validates that registration against
the recovery envelope, retrieves the root key from Secret Service, and remounts
the vault without retaining the recovery passphrase. Once the encrypted
database is open, the task manager attaches its vault journal, continues the
persisted monotonic sequence, and exposes any journal failure in runtime status.

## Security invariants

- Do not add a plaintext persistence fallback for SQLCipher or gocryptfs.
- Do not persist prompts, source content, conversations, task logs, or indexes
  before a verified `Vault` capability exists.
- Do not expose core commands over an HTTP listener.
- Tool paths must be canonicalized and checked outside the model.
- Generated code must execute only inside the constrained rootless containers
specified in `docs/IMPLEMENTATION_STATUS.md`.
