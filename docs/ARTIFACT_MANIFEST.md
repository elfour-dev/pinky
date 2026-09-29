# Signed runtime artifacts

R7 now has a core verifier for model and executable artifacts. A manifest is
trusted only after all of the following checks pass:

- the JSON schema is version `1` and contains no unknown fields;
- the artifact and licence URLs are HTTPS URLs;
- the declared byte size and lowercase SHA-256 are valid;
- the manifest signature verifies with Pinky's trusted Ed25519 public key; and
- the downloaded file matches the declared size and digest.

The signed envelope has this shape:

```json
{
  "key_id": "release-key-2026-01",
  "manifest": {
    "schema_version": 1,
    "artifact_id": "nomic-embed-text-q8",
    "kind": "model",
    "capability": "embeddings",
    "version": "1.5.0",
    "url": "https://models.example/nomic-embed-text.gguf",
    "sha256": "<64 lowercase hexadecimal characters>",
    "byte_size": 123456789,
    "license_url": "https://models.example/license",
    "runtime_version": "ollama-0.9",
    "context_length": 8192,
    "minimum_ram_bytes": 1073741824
  },
  "signature": "<base64 Ed25519 signature>"
}
```

The signature covers a domain-separated `pinky-artifact-manifest-v1` prefix
followed by the canonical JSON encoding of `manifest`. The verifier never
trusts a URL, model name, or executable path supplied by the model runtime.

The core verifier supports resolving an envelope's `key_id` only from a
bundled trusted public-key ring. A manifest with an unknown key ID, malformed
bundled key, or invalid signature is rejected before its URL or artifact
metadata can be used. The desktop route uses that verifier and must never
accept a public key supplied by a manifest, environment
variable, model runtime, or UI form. Development keys made with
[`artifact-dev.mjs`](../scripts/artifact-dev.mjs) are for exercising this
workflow only; they are not trusted by a private build until its owner
deliberately bundles the corresponding public key.

The desktop keyring resource is
`apps/desktop/src-tauri/trusted-artifact-keys.json`. Pinky is intended for a
personal owner and possibly a small trusted circle, not public distribution.
The private build bundles the owner's approved development public key. Future
owner keys use entries in the form
`{"key_id":"personal-owner-YYYY-NN","public_key_base64":"..."}`. The matching
private key stays outside the checkout and outside every build; do not add a
private key to that file. Every recipient must use a build containing the public
key that verifies the owner's manifests.

For Ollama-managed embedding models, R7 records the installed model digest from
the local `/api/tags` response. Ollama's bare 64-character form and the
`sha256:`-prefixed form are normalized and validated as lowercase hexadecimal
before a future signed allowlist may match them; the mutable model name alone is
insufficient attestation.

Installation follows at most three HTTPS redirects from the signed manifest
URL (needed by common official release hosts) into a uniquely named temporary
file in the destination directory, enforces the declared byte limit, verifies
that temporary file, flushes it, and then
renames it atomically. Official Qdrant Linux release archives are extracted
only after this verification; Pinky accepts exactly one regular top-level
`qdrant` payload and rejects links or extra entries. Invalid or incomplete
downloads are removed, and an existing destination is never overwritten. The implementation is in
`crates/pinky-core/src/artifact.rs`; desktop artifact selection, signed
owner-key configuration, and private-host onboarding passed R7 acceptance on
2026-09-29.

## Development-only Qdrant fallback

For development-only testing, `scripts/install-qdrant-dev.sh` pins the official Qdrant `v1.19.1`
x86-64 musl archive, checks the published byte size and SHA-256, and installs
the executable under the user data directory without root privileges. The
installer records `verification: development_pinned_archive`; this path is
intentionally excluded from the owner-signed private-host acceptance gate.
