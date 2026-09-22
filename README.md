# Pinky

**Pinky is the project's working title, not the final application name.** The
project is a private, source-grounded Linux desktop assistant being delivered in
six acceptance-gated stages; it is not yet a version-one release. Package,
executable, and repository identifiers retain the working title until a final
name is selected.

Planning and resumption documents:

- [Product and avatar identity](docs/IDENTITY.md)
- [Vault setup and usage guide](docs/VAULT_GUIDE.md)
- [Hands-on vault tutorial and example sources](docs/VAULT_TUTORIAL.md)
- [Cited local Q&A delivery phases](docs/CITED_QA_PHASES.md)
- [Local model and answer contract](docs/MODEL_CONTRACT.md)
- [Remaining development roadmap](docs/ROADMAP.md)
- [Offline self-development strategy](docs/OFFLINE_SELF_DEVELOPMENT.md)
- [R6 privacy inspection](docs/PRIVACY_INSPECTION.md)
- [Signed runtime artifact contract](docs/ARTIFACT_MANIFEST.md)
- [Next-session handoff](docs/NEXT_SESSION.md)
- [Implementation acceptance ledger](docs/IMPLEMENTATION_STATUS.md)

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
- an encrypted Tantivy lexical index, automatic rebuild from retained chunks,
  source search, and exact retained-version citation reopening;
- a three-region UI, live xterm event log, task controls, and a Three.js entity
  driven by task state, including reduced motion and a non-WebGL fallback.

Live core acceptance now covers real gocryptfs creation, Secret Service key
storage, unmounting, restart-time unlocking, and encrypted object recovery on
the target host. Native compilation, WebDriver end-to-end checks, Linux bundle
inspection, and clean Debian package installation now pass. The local
text-ingestion and lexical-search slices are usable from the Sources panel and
composer. When a local Ollama model is attached, **Ask** mode retrieves retained
passages and produces validated, cited answers; **Search** mode remains an
explicit lexical lookup. Hybrid vector retrieval is available as an opt-in
development path. Image metadata ingestion and a bounded supervised OCR worker
are now available in the core. Image rows can attach bounded OCR results as
encrypted searchable chunks, and image citations can open the exact retained
pixels. OCR can use a supervised ImageMagick preprocessing pass for photographed
or skewed scans. Automatic OCR tries all Tesseract page-segmentation modes with
bounded confidence adjustments and reports the attempted combination when it
fails. PDF extraction now runs through bounded supervised Poppler workers when
`pdftotext` and `pdftoppm` are installed; blank/image-only pages can be rendered
and OCRed with local Tesseract when available, with page-aware citations. The
full malformed/encrypted/oversized PDF acceptance matrix remains an R9 gate.
Office extraction is deliberately reserved for a final-stage
document-compatibility milestone. Image generation, automatic
research, generated-code containers, backup/restore, and the later release
gates remain unimplemented.
Retained-data operations
remain disabled while the vault is unavailable.

The Sources sidebar has been replaced by a bounded Asset Library. It keeps only
the asset count and navigation in the sidebar, while the centre panel provides
metadata search, type/state filters, keyset pagination, list/grid views, exact
retained-passage opening, image OCR actions, and per-detection or bulk OCR
deletion. Asset metadata remains inside
the mounted encrypted vault; image pixels are still loaded only on explicit
citation preview.

Core also contains the next retrieval foundation: a supervised, authenticated,
loopback-only Qdrant client and the specified reciprocal-rank fusion algorithm.
Hybrid retrieval remains dormant by default until a local Qdrant executable and
embedding model are explicitly configured; Pinky does not generate substitute
embeddings.

There is an opt-in R7 desktop bridge for development validation. Set all three
values before launching Pinky to enable hybrid retrieval in Ask and Search:

```bash
export PINKY_QDRANT_EXECUTABLE=/absolute/path/to/qdrant
export PINKY_OLLAMA_EMBEDDING_ENDPOINT=http://127.0.0.1:11434
export PINKY_OLLAMA_EMBEDDING_MODEL=nomic-embed-text
```

If Qdrant is not installed, the development-only installer pins the official
Linux x86-64 musl archive and verifies its byte size and SHA-256 before an
atomic user-local install:

```bash
scripts/install-qdrant-dev.sh
```

It prints the `PINKY_QDRANT_EXECUTABLE` export for the installed binary. This
is sufficient for local development validation, but it is deliberately not a
replacement for the signed artifact manifest and trusted-key onboarding
required by the R7 release gate.

The embedding endpoint must be an explicit loopback Ollama endpoint and the
model must be a locally installed embedding model. Pinky validates the
embedding model, starts Qdrant lazily on a random loopback port, backfills
current chunks from encrypted objects, and keeps the supervised sidecar alive
for the application session so subsequent searches do not restart it. If the
sidecar becomes unhealthy it is replaced, and it is stopped after five minutes
of inactivity. When these values are absent, the normal lexical retrieval path
remains in use.

For a persistent configuration, unlock the vault and choose **Configure hybrid
retrieval** in the runtime panel. Pinky verifies the embedding model and Qdrant
executable before storing their paths and names inside the encrypted SQLCipher
database. The environment variables remain useful for development-only
fallbacks and do not override a saved vault configuration.

The in-progress cited-Q&A milestone now attaches either an existing local
Ollama instance or an authenticated llama.cpp server. After vault unlock, open
**Attach local model** and select the provider:

- **Ollama (no key):** use `http://127.0.0.1:11434` and enter a model name shown
  by `ollama list`. Pinky confirms that the model is installed locally, uses
  is not a remote/cloud proxy, supports completion, uses GGUF, and advertises
  at least 2,048 context tokens.
- **llama-server (API key):** use `http://127.0.0.1:<port>` and its ephemeral
  64-character hexadecimal API key. Pinky checks readiness and verifies the key
  against the protected `/props` endpoint.

Both providers must use an explicit IPv4 loopback port; Pinky bypasses ambient
HTTP proxies. After attaching a model, unlock the vault, add at least one
retained source, switch the composer to **Ask**, and submit a question. Pinky
retrieves current retained evidence, sends only that evidence and the bounded
conversation window to the local model, validates the structured answer, and
renders exact retained-version citations. It does not use general-knowledge
fallback or public-web research yet.

Successful Ollama attachments are saved inside the encrypted vault alongside
the hybrid settings. On the next unlock Pinky reconnects to that endpoint and
model automatically; the Ollama endpoint is stored without an API key. Detach
the model to clear the saved Ollama preference. llama-server API keys remain
session-only and must be entered again after restart.

## Setup on Debian or Ubuntu

The commands below describe a fresh development installation. Do not begin with
`npm run tauri dev` until both Node.js and Rust/Cargo are available.

### Automated bootstrap (recommended)

The repository includes an idempotent Debian/Ubuntu bootstrap for development.
It installs the native Tauri, vault, Podman, Vulkan, WebDriver, Tesseract,
ImageMagick OCR, and Poppler PDF packages, installs or verifies the stable Rust toolchain, fetches locked
Rust crates, installs the locked desktop JavaScript dependencies, and installs
`tauri-driver` for native end-to-end tests:

```bash
./scripts/bootstrap-dev.sh
```

Use `./scripts/bootstrap-dev.sh --skip-e2e` when the WebDriver acceptance suite
is not needed. The script requires `sudo` for system packages and does not
download Ollama, model files, or Qdrant; those optional runtimes are large and
must be installed and configured separately. It supports Debian-family systems
only. English Tesseract language data (`eng`) and ImageMagick are installed for the built-in
image OCR workflow. The remaining sections document the equivalent manual
setup.

### Manual setup

#### 1. Install native dependencies

Install the current Tauri 2 Linux build dependencies together with the tools
required to create and unlock Pinky's encrypted vault and run local image OCR:

```bash
sudo apt update
sudo apt install -y \
  build-essential \
  curl \
  file \
  gocryptfs \
  imagemagick \
  libayatana-appindicator3-dev \
  libsecret-tools \
  libssl-dev \
  libwebkit2gtk-4.1-dev \
  libxdo-dev \
  librsvg2-dev \
  pkg-config \
  poppler-utils \
  tesseract-ocr \
  tesseract-ocr-eng \
  wget
```

Rootless Podman and Vulkan tooling are not required for the functionality
currently implemented, but later Pinky stages expect them:

```bash
sudo apt install -y podman vulkan-tools
```

The upstream [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
are the authority for distribution-specific package changes.

#### 2. Install Rust and Cargo

Tauri requires Rust. Install the stable toolchain with the official `rustup`
installer; Cargo is included:

```bash
curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
rustup default stable
```

You may inspect the installer at <https://sh.rustup.rs> before running it. A new
terminal normally loads Cargo automatically. If `cargo: command not found`
appears in the current terminal, run `source "$HOME/.cargo/env"` again.

Verify the installation:

```bash
rustc --version
cargo --version
```

#### 3. Install Node.js and project dependencies

Install a supported Node.js LTS release from <https://nodejs.org/> if Node is
not already present. Pinky requires Node.js 20 or newer.

```bash
node --version
npm --version

cd apps/desktop
npm install
```

Run `cd apps/desktop` from the root of your Pinky checkout.

The Tauri CLI is already a locked npm development dependency. Do not install a
second global copy merely to run this project.

#### 4. Verify and launch

From the desktop application directory:

```bash
npm test
npm run build
npm run tauri dev
```

The first native launch may take several minutes while Cargo downloads and
compiles Rust dependencies. The application window should then open. Pinky
needs a normal graphical desktop session with D-Bus and Linux Secret Service in
order to create or unlock its vault.

The expression debug lab is hidden during normal launches. To opt into the
temporary ALMA state controls, pass the explicit application argument:

```bash
# Development launch (the second `--` forwards the argument to Pinky)
npm run tauri dev -- -- --expression-debug

# Existing native debug build, when still in apps/desktop
../../target/debug/pinky-desktop --expression-debug

# The same native build when launched from the repository root
./target/debug/pinky-desktop --expression-debug
```

To run the Rust tests separately from the repository root:

```bash
cd ../..
cargo test --workspace
```

To run the deterministic offline Pinky Lite acceptance scenario independently:

```bash
cargo test -p pinky-core --test pinky_lite_offline
```

This uses the bundled Project Alder tutorial sources and a local fixture
provider; it does not contact Ollama or the public internet. The configured
Ollama target-host scenario remains an opt-in release check.

### Troubleshooting setup

- `cargo: command not found`: install Rust with `rustup`, then run
  `source "$HOME/.cargo/env"` or open a new terminal.
- `failed to get cargo metadata: No such file or directory`: Cargo is missing
  from the environment used to launch npm; fix `PATH` as above.
- Vault setup reports missing prerequisites: verify `command -v gocryptfs` and
  `command -v secret-tool` both print paths.
- Secret Service or keyring errors: launch Pinky inside your logged-in graphical
  desktop session rather than a bare SSH or headless shell.
- WebKit, GTK, linker, or `pkg-config` errors: reinstall the native packages in
  step 1 and compare them with Tauri's current prerequisites page.

With a desktop Secret Service session and `/dev/fuse` access, run the opt-in
live vault acceptance test with:

```bash
cargo test -p pinky-core --test live_onboarding -- --ignored
```

After unlocking the vault, choose **Add source**, enter an approved directory,
and enter the absolute path of a file inside it. Pinky archives the exact opened
file in the encrypted vault and shows ingestion progress in the task panel.
Enter terms in the centre composer to search retained passages. Open a result's
citation to inspect the exact archived source version and provenance.
After the first file from an approved directory is retained, Pinky watches that
directory recursively and automatically discovers new regular files once they
remain stable. The new-file ingestion task then refreshes the Sources list.

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
