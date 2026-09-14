# Next-session handoff

Recorded: 2026-09-14

## Active objective

Deliver the cost-first Pinky Lite milestone: useful local question answering
over encrypted retained sources, with exact citations, visible cancellation,
and encrypted conversations. The complete version-one specification remains
the longer-term scope; embeddings, Qdrant, web research, and generation are not
prerequisites for Pinky Lite.

The authoritative delivery sequence, contracts, and gates are in
[`CITED_QA_PHASES.md`](CITED_QA_PHASES.md).

## Last committed baseline

```text
8e47e57 feat: add hybrid retrieval foundation and vault tutorials
```

That baseline includes encrypted vault setup and restart unlock, approved local
text ingestion, stable-file watching, source versioning, lexical retrieval,
exact retained citations, task journaling and cancellation, the desktop shell,
the Qdrant/RRF foundation, user guides, and tutorial sources.

## Current in-progress phase

Q1A, the local model core connection contract, has been implemented but is not
committed:

- `crates/pinky-core/src/llama.rs` accepts only an explicit
  `http://127.0.0.1:<port>` origin;
- it requires a 256-bit hexadecimal bearer token and bypasses ambient proxies;
- secrets are redacted from diagnostics;
- the bounded readiness request supports cooperative cancellation;
- the current upstream llama.cpp `{"status":"ok"}` health response is strictly
  validated; and
- deterministic tests verify endpoint rejection, key validation, redaction,
  authorization-header transmission, readiness parsing, and pre-cancellation.

The explicit runtime boundary is complete for the Ollama-first delivery track:

- the desktop exposes a vault-only attach form;
- endpoint and API key values remain ephemeral;
- `/props` proves the token on a protected endpoint;
- model path, slot count, and a minimum 2,048-token context are validated;
- attached model name, context size, errors, and task state are surfaced; and
- detach or vault loss drops the client and zeroizes its token.

The explicit attach screen also supports a local Ollama provider:

- it accepts only an explicit `http://127.0.0.1:<port>` origin and bypasses
  ambient proxies;
- it follows Ollama's local no-authentication API and sends no authorization
  header;
- the user selects an installed model by name;
- `/api/version`, `/api/tags`, and `/api/show` validate the runtime, local model
  presence, absence of a remote/cloud target, completion capability, GGUF
  format, and minimum 2,048-token context; and
- deterministic loopback tests cover no-key requests, model validation, and
  cancellation.

Native desktop state tests cover lock, concurrency, success, failure, detach,
and vault loss. The opt-in live test passed on 2026-09-14 through the user's SSH
tunnel at `127.0.0.1:11435`, probing Ollama 0.33.2 and `qwen3.5:9b` with 262,144
advertised context tokens. Managed llama-server remains a non-blocking
compatibility track.

The health endpoint is public by llama.cpp design, so readiness alone is never
treated as proof of authentication.

Strict Clippy exposed two baseline design warnings during Q1A. They were fixed
without changing behavior by grouping task-event transition fields into a
typed update and moving the desktop test module after runtime items.

## Verification completed

- `cargo fmt --all --check`: passed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- `cargo test --workspace`: 63 core and 6 desktop tests passed
- opt-in live Ollama target-host test: passed separately
- opt-in live Secret Service/FUSE and Ollama tests: ignored in the ordinary
  regression suite as designed
- `npm test`: 3 frontend tests passed
- `npm run build`: passed with the existing Vite chunk-size warning
- `git diff --check`: passed

The deterministic fake llama-server test needs loopback permission; the
restricted filesystem sandbox returned `EPERM`, and the same suite passed when
run with explicit local-loopback permission.

## Next implementation slice: R2 Ollama generation transport

The user has connected Pinky to an Ollama model through the explicit attach
path. Follow `CITED_QA_PHASES.md` and implement R2 next:

1. Add the provider-neutral inference boundary.
2. Implement bounded, cancellable Ollama `/api/chat` structured output.
3. Reject response model mismatches and remote/cloud metadata.
4. Cover the protocol and every defined failure with a deterministic fake
   Ollama server.
5. Run a non-sensitive target-host schema smoke test.

Do not display or persist generated text during R2. Supervised llama-server
launch remains a parallel compatibility track and no longer blocks the
Ollama-first Pinky Lite route. Do not download a model or executable implicitly.

## Orientation commands

```bash
git status --short
git diff --check
git diff -- crates/pinky-core/src/llama.rs crates/pinky-core/src/task.rs
sed -n '1,260p' docs/CITED_QA_PHASES.md
/home/mickey/.cargo/bin/cargo test --workspace
```

Do not commit unless the user explicitly asks for a commit.
