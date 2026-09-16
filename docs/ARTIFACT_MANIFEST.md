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

Installation streams the exact HTTPS manifest URL without following redirects
into a uniquely named temporary file in the destination directory, enforces
the declared byte limit, verifies that temporary file, flushes it, and then
renames it atomically. Invalid or incomplete downloads are removed, and an
existing destination is never overwritten. The implementation is in
`crates/pinky-core/src/artifact.rs`; desktop artifact selection and signed
release-key distribution remain gated R7 onboarding work.
