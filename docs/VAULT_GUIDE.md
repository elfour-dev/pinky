# Pinky vault setup and usage

This guide covers the functionality available in the current development build:
creating and unlocking an encrypted vault, retaining supported local files,
searching their text, and opening exact citations. Conversational AI, generated
answers, PDF and Office extraction, web research, and backup/restore are not yet
available.

For a practical walkthrough with ready-made Markdown, JSON, CSV, and log files,
follow the [hands-on vault tutorial](VAULT_TUTORIAL.md).

## What the vault protects

Pinky will not retain sources, indexes, task journals, or other private data
until its encrypted vault is mounted. The vault uses:

- **gocryptfs** for the encrypted filesystem;
- **Linux Secret Service** for the key used during normal daily startup; and
- a recovery passphrase to protect a separate recovery envelope.

The encrypted directory contains the data safe to keep at rest. The mount
directory exposes its readable contents only while Pinky has the vault open.
Do not edit either directory manually.

### What goes into the vault today

When you add a supported local file, Pinky retains more than a link to the
original. The current vault contains:

| Retained item | Purpose |
| --- | --- |
| Original source bytes | An exact archived copy of the file Pinky opened. The original file is copied, not moved or modified. |
| Extracted text | Normalized text used for indexing and passage retrieval. The archived original remains unchanged. |
| Chunks | Overlapping, searchable passages with headings, order, byte and character offsets, line coordinates, and token counts. |
| Source records | The source name, canonical path, approved directory, type, status, refresh policy, and current-version pointer. |
| Source versions | A durable history connecting each ingestion to its archived original, extracted text, MIME type, size, retrieval time, processing state, and citation map. |
| Lexical search index | A Tantivy index of retained chunks used by the centre search field. It can be rebuilt from retained text if necessary. |
| Metadata database | A SQLCipher database holding relationships, version pointers, checksums, object reference counts, tasks, and schema versions. |
| Task event journals | Compressed histories of durable operations such as ingestion and retrieval, including their state and errors. |
| Integrity metadata | SHA-256 checksums, MIME types, compressed and uncompressed sizes, and compression levels used to verify retained objects. |

Payloads are stored as Zstandard-compressed, content-addressed objects. If two
retained items have identical bytes, they can share one stored object instead
of consuming space twice. Reads verify the object's checksum before returning
its content.

The database schema also reserves places for conversations, messages, topics,
claims, evidence, generated artifacts, and vector identifiers. Those records
will become useful in later stages; the current UI does not yet create
conversational AI, dossiers, generated artifacts, or embeddings.

### What remains outside the mounted vault

Some minimum bootstrap information must be available before the vault can be
opened:

- the **encrypted data directory**, containing gocryptfs ciphertext and the
  passphrase-protected recovery envelope;
- an owner-only **vault registration**, containing the vault UUID and its
  configured paths, but not the root key or retained source contents; and
- the random root key held by **Linux Secret Service** for automatic unlock.

The readable mount is temporary and exists only while gocryptfs has the vault
open. Your original source files also remain in their original locations and
are not encrypted merely because Pinky retained copies of them. Development
terminal output from `npm run tauri dev` is not a vault backup and may be
visible in the terminal session.

## 1. Install and verify the prerequisites

On Debian or Ubuntu:

```bash
sudo apt update
sudo apt install -y gocryptfs libsecret-tools
```

Check that both commands are available:

```bash
command -v gocryptfs
command -v secret-tool
```

Each command should print a path. Pinky must run inside your normal graphical
desktop session so it can communicate with the desktop keyring over D-Bus.
Headless and bare SSH sessions generally cannot provide this.

The full development environment, including Rust, Node.js, and Tauri's native
libraries, is documented in the [README](../README.md#setup-on-debian-or-ubuntu).

## 2. Start Pinky

From the repository checkout:

```bash
cd apps/desktop
npm run tauri dev
```

Wait for the native Pinky window. A browser preview cannot create or use a
vault.

## 3. Create a new vault

1. Select **Start setup** in the centre of the Pinky window.
2. Leave **Encrypted data directory** and **Unlocked mount directory** at their
   prefilled defaults unless you have a specific reason to relocate them.
3. Choose a unique recovery passphrase of at least 12 characters. A longer
   multi-word passphrase is preferable.
4. Enter it again under **Confirm passphrase**.
5. Store that passphrase in a password manager or another safe place before
   continuing. Pinky does not retain it.
6. Select **Create vault** and allow your desktop keyring request if one appears.
7. Wait for **Vault created and mounted**, then select **Continue**.

Successful setup shows **Vault unlocked — Encrypted storage available** in the
lower-left status card. The runtime panel should show the task journal as
`durable` and the file watcher as `watching`.

Setup generates a random vault key. Secret Service stores that key for ordinary
startup; the recovery passphrase protects the recovery envelope shown after
creation. Losing both the Secret Service entry and the passphrase makes the
encrypted data inaccessible.

> **Current recovery limitation:** the recovery envelope is created, but the
> desktop does not yet provide a passphrase-recovery screen. Preserve the
> passphrase and the entire encrypted vault together for use by the future
> recovery workflow. A passphrase by itself is not a backup of the data.

### If vault creation reports an error

| Message | What it means and what to do |
| --- | --- |
| `vault paths must be absolute, normalized, distinct directories` | One path is relative, contains `.` or `..`, resolves through a symlink, overlaps the other path, or is the filesystem root. Restore the prefilled defaults, or choose two separate absolute paths that are not nested inside one another. |
| `vault setup requires an empty destination` | One selected directory already contains files. Do not point Pinky at an existing data directory. Choose new empty directories; do not erase an old encrypted vault merely to make the message disappear. |
| `required program is unavailable: gocryptfs` | Install `gocryptfs`, verify `command -v gocryptfs`, and restart Pinky from a terminal that has the same `PATH`. |
| `required program is unavailable: secret-tool` | Install `libsecret-tools`, verify `command -v secret-tool`, and restart Pinky. |
| `a vault is already registered; unlock or repair it instead` | Pinky found an existing registration and intentionally refuses to overwrite it. Close setup and use **Retry unlock**. Preserve the old vault while diagnosing it. |
| `Secret Service storage failed` | The desktop keyring did not accept the new key. Confirm that Pinky is running in your graphical login session, unlock the keyring, and retry setup. |
| `SQLCipher is unavailable; refusing to create an unencrypted database` | The application was built without working SQLCipher support. Rebuild using the documented dependencies; Pinky will not fall back to a plaintext database. |

Longer messages can be prefixed with text such as `vault setup worker failed:`.
The useful cause is usually the final message described above.

## 4. Add a local source

1. Select **Add source** under **Sources**, or **Attach source** below the centre
   input.
2. Under **Approved directory**, enter an absolute directory path, for example
   `/home/you/Documents/project-notes`.
3. Under **Source file**, enter the absolute path to one file inside that
   directory, for example `/home/you/Documents/project-notes/decisions.md`.
4. Select **Archive and extract**.
5. Watch the ingestion task in the right-hand operations panel. The source
   appears in the left panel after the task completes.

The source file must resolve inside the approved directory. Pinky rejects
directories, devices, sockets, and symlinks that escape the approved root.

For example, these paths are compatible:

```text
Approved directory: /home/you/Documents/project-notes
Source file:        /home/you/Documents/project-notes/decisions.md
```

These are not compatible because the file is in `Downloads`, not inside the
approved `Documents/project-notes` directory:

```text
Approved directory: /home/you/Documents/project-notes
Source file:        /home/you/Downloads/decisions.md
```

### If adding a source reports an error

| Message | What it means and what to do |
| --- | --- |
| `source path is outside the approved root` | After resolving both paths, the source is not a descendant of the approved directory. Select an approved directory that genuinely contains the file, or select a file inside the current root. A similarly named path elsewhere is not enough. Pinky also produces this error when a symlink inside the root points outside it. |
| `approved root must be an existing directory` | The approved path does not exist or points to a file. Create or select the containing directory and use its absolute path. |
| `source must be a regular file` | The source is a directory, socket, device, FIFO, or another special entry. Select one ordinary file. |
| `ingestion I/O error: No such file or directory` | A path is misspelled, relative to somewhere unexpected, or the file moved before Pinky opened it. Re-enter both absolute paths and try again. |
| `ingestion I/O error: Permission denied` | Your logged-in user cannot read the source or traverse one of its parent directories. Correct the host file permissions or choose a readable source; do not run Pinky as root. |
| Source becomes `unsupported` | Pinky archived the original successfully but cannot extract searchable text from that format yet. Convert a copy to a currently supported text format if you need to search it now. |
| Task shows `cancelled` | The operation was stopped before a new source version became current. Start the ingestion again if the source is still wanted. |

To see how Linux resolves uncertain paths, run:

```bash
realpath -- "/path/entered/as/approved-directory"
realpath -- "/path/entered/as/source-file"
```

The second result must begin with the complete first path followed by `/`.
Do not use a broader approved directory than you are comfortable granting just
to silence the error.

Currently extractable formats are UTF-8 text, Markdown, logs, source code, JSON,
YAML, XML, HTML, and CSV. Other files are retained safely but marked
`unsupported`; image metadata/OCR is the next planned ingestion phase. PDF and
Office extraction are intentionally deferred until after image support, while
web ingestion remains a later phase.

Pinky watches successfully ingested local files. After a file becomes stable,
an edit creates a new retained and searchable version. If replacement indexing
fails, the previous searchable version remains available. Deleting the original
marks the source as missing but does not delete its retained history.

## 5. Search and open citations

1. Type keywords or a phrase into **Search your retained sources…**.
2. Press **Enter** or select the arrow button.
3. Review the matching passages shown in the centre panel.
4. Select **Open retained citation** on a result.

The citation viewer displays the retained passage, its line coordinates,
retrieval time, original path, MIME type, and a stable `pinky://` citation URI.
It opens the archived source version, so later edits to the original file do
not change old citations.

The centre input currently performs lexical evidence search. It does **not**
answer questions or conduct a chat yet. For example, search for distinctive
terms present in a retained document rather than asking Pinky to summarize it.

### If search or citation opening reports an error

| Message or behaviour | What it means and what to do |
| --- | --- |
| No matching passages | Lexical search did not find those words in an active extracted chunk. Try fewer or more distinctive terms, check that ingestion completed, and remember that unsupported files have no searchable chunks. |
| `vault unavailable` | The encrypted mount was lost or locked during the operation. Return to the vault status, restore the mount with **Retry unlock**, and search again. |
| `retrieval index error` | The local Tantivy index could not be read or updated. Restart Pinky once so it can reopen or rebuild the index from retained chunks. Preserve the vault and report the full error if it repeats. |
| `invalid citation URI` | The supplied `pinky://` value is malformed. Open the citation from a current search result rather than editing or typing the URI. |
| `citation does not refer to a retained chunk` | The URI is structurally valid but its exact chunk is not present in this vault. Re-run the search in the vault that originally produced it. |

### Make practical use of retained knowledge

The current build works best as a private, versioned evidence library. A useful
workflow is:

1. Create a dedicated source directory containing the notes, decisions,
   specifications, logs, or code you want Pinky to retain.
2. Prefer descriptive filenames and structured text with headings. This makes
   the source list and citation passages easier to understand.
3. Add each file with the narrowest sensible approved directory. Approving a
   project directory is clearer than approving your whole home directory.
4. Confirm that ingestion completes and that the source shows an active chunk
   count in the left panel.
5. Search using distinctive names, error messages, identifiers, or phrases
   expected to occur in the material.
6. Open the retained citation instead of relying only on the result preview.
   Check its coordinates, retrieval time, and original source path.
7. Edit the original file normally. Pinky's watcher will retain a new version
   after the file stabilizes; existing citation URIs continue to identify the
   older archived version.

Example uses available now include locating a past decision in Markdown notes,
finding where an identifier appears across source files, retaining successive
versions of a changing specification, and reopening the exact evidence behind
a search result.

Keep the original files. Pinky is currently an encrypted retained-knowledge
store, not a supported backup system, document editor, or source-control
replacement.

## 6. Unlock on later launches

On startup Pinky reads its registration, asks Linux Secret Service for the
stored key, and mounts the vault automatically. You do not normally enter the
recovery passphrase again.

If the status remains **Vault registered** or reports that the vault is locked:

1. Confirm you launched Pinky from the same logged-in desktop account and
   graphical session used during setup.
2. Check `command -v gocryptfs` and `command -v secret-tool` again.
3. Unlock your desktop keyring if it prompts you.
4. Select **Retry unlock**.

If it still fails, preserve the encrypted directory and recovery envelope; do
not delete or reinitialize them while investigating. Common causes are a locked
or replaced desktop keyring, a missing Secret Service entry, an unavailable
FUSE mount, or moved vault paths.

### If unlocking reports an error

| Message | What it means and what to do |
| --- | --- |
| `no valid vault registration was found` | Pinky has no usable bootstrap record for a vault. This is expected before first setup. If a vault previously existed, do not create over its directories; locate or restore its matching registration first. |
| `Secret Service lookup failed` | The registered vault's root key was not returned by the current desktop keyring. Unlock the keyring and retry from the original desktop account. If its entry was deleted, automatic unlock cannot work. |
| `the vault is already open in another Pinky process` | Another Pinky instance owns the mount. Use that instance or close it cleanly before retrying. |
| `vault is not a verified gocryptfs mount` | The mount path exists, but it is not currently the verified encrypted filesystem. Close other Pinky instances and retry. Do not place ordinary files in the mount directory. |
| `vault registration is invalid` | The registration, recovery envelope, vault UUID, or configured paths no longer agree. Preserve all related files and diagnose the mismatch instead of editing the JSON manually. |
| `vault unlock worker failed` or `vault unlock task ended without a result` | The background unlock operation stopped unexpectedly. Restart Pinky once; if it repeats, retain the complete error and terminal output for diagnosis. |

The recovery passphrase cannot currently repair a missing Secret Service entry
through the desktop UI. Do not repeatedly guess passphrases or delete encrypted
data; preserve the vault for the planned recovery interface.

## 7. Operational controls and current limits

- Active ingestion and system-check operations appear in the right panel.
- Use **Pause**, **Resume**, or **Stop** when those controls are offered.
- **Stop all** cancels all currently cancellable operations.
- The animated ASCII entity is named **ALMA**, short for **Archived Local Memory
  Assistant**. ALMA reflects actual task state; she is the visible face of the
  application, not a separate AI model or evidence source. Pinky remains the
  application's working title until a final product name is selected.
- **New conversation**, dossiers, workspace automation, generated content, and
  public-web research are present only as future-facing UI or planned stages.
- The current build has no supported backup, restore, source purge, or vault
  recovery interface. Do not treat it as the only copy of important material.

For the precise implementation boundary, see the
[implementation acceptance ledger](IMPLEMENTATION_STATUS.md).

## Glossary

### Approved directory (approved root)
The directory boundary you grant Pinky for one local ingestion request. The
selected source must resolve inside it.

### Canonical path or URI
A normalized, unambiguous identifier for a source. For a local file, Pinky uses
the resolved absolute path to prevent path tricks and symlink escapes.

### Chunk
A searchable passage cut from extracted text. Chunks overlap slightly so
useful context is less likely to be lost at a boundary.

### Citation
A reference to an exact retained passage and source version. Pinky citations
use a `pinky://source/.../version/...` URI and include source coordinates.

### Ciphertext
Encrypted data that is unreadable without the correct key. This is what is
stored in the encrypted data directory while the vault is locked.

### Content-addressed object
A stored payload named by the SHA-256 checksum of its uncompressed bytes.
Identical payloads have the same address, enabling deduplication and integrity
checks.

### Deduplication
Reusing one stored object when identical content is retained more than once.
Metadata and versions may still be distinct even when their payload is shared.

### Encrypted data directory (cipher directory)
The at-rest gocryptfs directory. It contains encrypted names and content plus
the recovery envelope. It is not the directory to browse for readable files.

### Extracted text
Text derived from an archived source for chunking and search. It does not
replace or alter the retained original bytes.

### File watcher
The background component that notices changes to successfully ingested local
files and creates new retained versions after they become stable.

### gocryptfs
The filesystem encryption tool Pinky uses to turn the encrypted data directory
into a readable mounted view after successful unlock.

### Lexical search
Word-based retrieval over retained text. This is the search available today;
semantic vector search and AI-composed answers are not connected yet.

### Linux Secret Service
The desktop keyring interface Pinky uses to store and retrieve the random root
key for normal automatic unlocking.

### MIME type
A label describing a payload's data format, such as `text/markdown` or
`application/json`. Pinky uses it when extracting and compressing data.

### Mount directory
The temporary readable view of the vault created by gocryptfs. A directory
existing at that path does not by itself mean the encrypted vault is mounted.

### Recovery envelope
An encrypted copy of the random root key protected by the recovery passphrase.
It is stored alongside the encrypted vault and is not a copy of the retained
source data.

### Recovery passphrase
The user-created phrase used to decrypt the recovery envelope. Pinky does not
store it, and the current desktop build does not yet expose the recovery flow.

### Root key
The random 256-bit secret generated at setup. Pinky derives separate filesystem
and database keys from it; Linux Secret Service holds it for daily unlock.

### SHA-256 checksum
A 64-character content fingerprint Pinky uses to address and verify stored
objects. It detects accidental corruption but is not itself encryption.

### Source
A logical retained item, such as one approved local file. A source can have
many versions over time.

### Source version
One archived state of a source, with its original bytes, extracted content,
metadata, processing result, chunks, and citations.

### SQLCipher
The encrypted SQLite implementation used for Pinky's metadata database inside
the mounted vault.

### Tantivy
The local search engine library used to build Pinky's current lexical index.

### Task journal
The durable event history of an operation. It lets Pinky expose progress and
recognize work interrupted by an application restart.

### Vault registration
The small owner-only bootstrap record that tells Pinky which vault UUID and
paths to open. It does not contain the root key or retained source contents.

### Zstandard (Zstd)
The lossless compression format used for content-addressed objects before the
mounted vault's filesystem encryption is applied.
