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
- monotonic task events, task trees, pause checkpoints, cooperative cancellation,
  and a ten-second hard abort deadline for in-process workers;
- a three-region UI, live xterm event log, task controls, and a Three.js entity
  driven by task state, including reduced motion and a non-WebGL fallback.

Vault onboarding, ingestion, retrieval, models, research, generated-code
containers, image generation, backup/restore, and release packaging remain gated
work. The UI intentionally refuses conversations and retained-data operations
while the vault is unavailable.

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

## Security invariants

- Do not add a plaintext persistence fallback for SQLCipher or gocryptfs.
- Do not persist prompts, source content, conversations, task logs, or indexes
  before a verified `Vault` capability exists.
- Do not expose core commands over an HTTP listener.
- Tool paths must be canonicalized and checked outside the model.
- Generated code must execute only inside the constrained rootless containers
specified in `docs/IMPLEMENTATION_STATUS.md`.
