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
- approved-root ingestion for UTF-8 text, Markdown, logs, source code, JSON,
  YAML, XML, HTML, and CSV, with encrypted originals, versioned metadata,
  overlapping chunks, deduplication, and symlink-escape protection;
- automatic stable-file refresh after vault unlock, preserving the previous
  version on failed replacement and marking deleted files as missing;
- a three-region UI, live xterm event log, task controls, and a Three.js entity
  driven by task state, including reduced motion and a non-WebGL fallback.

Live core acceptance now covers real gocryptfs creation, Secret Service key
storage, unmounting, restart-time unlocking, and encrypted object recovery on
the target host. Native compilation, WebDriver end-to-end checks, Linux bundle
inspection, and clean Debian package installation now pass. The first local
text-ingestion slice is usable from the Sources panel. Additional extractors,
retrieval indexes, models, research, generated-code containers, image
generation, backup/restore, and the later release gates remain unimplemented.
Chat controls are visibly disabled until retrieval and the local model runtime
exist; retained-data operations remain disabled while the vault is unavailable.

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

With a desktop Secret Service session and `/dev/fuse` access, run the opt-in
live vault acceptance test with:

```bash
cargo test -p pinky-core --test live_onboarding -- --ignored
```

Run the desktop application with:

```bash
cd apps/desktop
npm run tauri dev
```

After unlocking the vault, choose **Add source**, enter an approved directory,
and enter the absolute path of a file inside it. Pinky archives the exact opened
file in the encrypted vault and shows ingestion progress in the task panel.

On Debian 13, install the native build and headless WebDriver prerequisites
before running the Stage 1 desktop acceptance suite:

```bash
sudo apt-get install -y pkg-config libdbus-1-dev libwebkit2gtk-4.1-dev \
  libgtk-3-dev librsvg2-dev patchelf webkit2gtk-driver xvfb
cargo install tauri-driver --locked
cd apps/desktop
npm run test:e2e
```

The E2E runner uses an isolated XDG profile and drives the compiled Tauri
application through WebKit WebDriver. It verifies the native three-region
shell, the default-vault-path command, task events, and cancellation. It has no
third-party Node dependencies. Linux bundle creation and inspection run with:

```bash
npm run bundle:linux
npm run verify:bundle
```

The clean-machine version of these checks is defined in
`.github/workflows/stage-one.yml`. After building the bundles, it can also be
run locally with `npm run test:clean-install`; this installs the `.deb` and
launches Pinky as an unprivileged user in a pinned Debian 13 container.

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
