# Development artifact-signing tutorial

This tutorial lets a new contributor practise Pinky's signed-artifact workflow
without creating an owner credential. It creates a **development
key** outside the source checkout, signs a manifest, and verifies that the
signature has not changed. It does not install an artifact or make a release.

## Fastest first run

To see the complete development flow without choosing a real artifact, run:

```bash
node scripts/artifact-dev.mjs demo
```

It creates a development key, harmless sample file, unsigned manifest, signed
manifest, and verification result automatically. It downloads and installs
nothing. The command prints one outside-the-checkout directory containing all
of those practice materials; delete that directory when you are finished.

## Why a signing key is needed

A hash tells Pinky that a downloaded file matches a particular hash. A signed
manifest additionally tells Pinky which hash, URL, purpose, and resource
requirements Pinky maintainers approved. The private key makes that approval;
the public key lets Pinky check it. Anyone may have the public key. Only the
designated private owner may access the private key.

Do not download a public key from a model provider and do not put a private key
in Git, a `.env` file, a chat message, or a release artifact.

## 1. Create a development key

From the repository root, run:

```bash
node scripts/artifact-dev.mjs init
```

The command uses `$XDG_STATE_HOME/pinky/artifact-dev` (usually
`~/.local/state/pinky/artifact-dev`) so neither key is created in the checkout.
It prints two paths:

- `development-key-1.private.pem` is secret. Keep it private and delete it
  after the exercise if it is no longer useful.
- `development-key-1.public.json` is safe to share. Its `key_id` and raw
  base64 public key are the form Pinky needs to embed in a future release.

## 2. Describe one artifact

First obtain an artifact from its official project and inspect its licence. Do
this only for a file you are permitted to use. The helper calculates the file
size and SHA-256 while creating an unsigned manifest; the URLs, version,
capability, and resource requirements remain a human review responsibility:

```bash
node scripts/artifact-dev.mjs create-manifest \
  --artifact /absolute/path/to/qdrant \
  --artifact-id example-qdrant \
  --kind executable \
  --capability vector_database \
  --version 1.2.3 \
  --url https://releases.example.invalid/example-qdrant \
  --license-url https://example.invalid/license \
  --runtime-version linux-x86_64 \
  --context-length none \
  --minimum-ram-bytes 1073741824 \
  --out /absolute/path/unsigned-artifact.json
```

It produces JSON in this shape:

```json
{
  "schema_version": 1,
  "artifact_id": "example-qdrant",
  "kind": "executable",
  "capability": "vector_database",
  "version": "1.2.3",
  "url": "https://releases.example.invalid/example-qdrant",
  "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "byte_size": 123456,
  "license_url": "https://example.invalid/license",
  "runtime_version": "linux-x86_64",
  "context_length": null,
  "minimum_ram_bytes": 1073741824
}
```

The helper requires HTTPS, exactly these fields, lowercase SHA-256, and no
unexpected manifest fields. For a model, set `kind` to `model`, use capability
`embeddings`, and provide its non-zero `context_length`.

## 3. Sign and verify the manifest

Substitute the paths printed in step 1:

```bash
node scripts/artifact-dev.mjs sign \
  --key /absolute/path/development-key-1.private.pem \
  --key-id development-key-1 \
  --manifest /absolute/path/unsigned-artifact.json \
  --out /absolute/path/signed-artifact.json

node scripts/artifact-dev.mjs verify \
  --public-key /absolute/path/development-key-1.public.json \
  --manifest /absolute/path/signed-artifact.json
```

The second command must report that the manifest is valid. Change one
character in the signed manifest and run it again; verification must fail.

## Moving from development to a private build

Pinky is intended for personal use and a small trusted circle, not public
distribution. Before enabling artifact onboarding in a private build, its owner
must establish a key policy:

1. Generate an Ed25519 private key in secure local custody (an offline system,
   hardware token, or password-protected local secret store).
2. Assign a stable `key_id`; bundle only its public key with Pinky.
3. Review each upstream artifact URL, licence, version, size, and hash before
   signing a manifest with the protected key.
4. Embed the approved public key(s) in Pinky and have the desktop onboarding
   path reject all untrusted keys and unsigned manifests.
5. Rotate keys by making the next trusted public key available in the private
   build before using it to sign manifests; retain an old public key only while
   its signed artifacts remain
   supported.

To make a chosen owner key trusted, run:

```bash
node scripts/artifact-dev.mjs trust-key \
  --public-key ~/.local/state/pinky/artifact-dev/development-key-1.public.json \
  --keyring "$PWD/apps/desktop/src-tauri/trusted-artifact-keys.json"
```

This is an intentional source-controlled trust-root change; do it only after
the owner has selected the key, then rebuild Pinky. The command rejects a
duplicate key ID and never reads or writes the `.private.pem` file. A person
you share Pinky with needs the rebuilt application, not the private key.
