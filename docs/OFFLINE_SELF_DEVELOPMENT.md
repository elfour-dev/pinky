# Offline self-development strategy

Status: proposed cost-control direction, recorded 2026-07-28 for a later
implementation session.

## Objective

Reduce paid-model usage by giving Pinky a constrained local development loop.
The intended outcome is not an unsupervised system that redesigns itself. It is
an offline worker that completes small, testable tasks inside an approved
checkout and escalates uncertainty instead of guessing.

## Feasibility

The target computer has 27 GiB RAM and AMD integrated graphics. That is enough
for a quantized local model to perform bounded editing, testing, documentation,
and routine refactoring. Local inference will be slower and less reliable than
a strong hosted coding model. Architecture, cryptography, permissions, process
containment, destructive migrations, and ambiguous failures must remain review
gates.

Pinky cannot perform this loop yet. The missing bootstrap consists of a local
model runtime, a developer task queue, canonical workspace tooling, recovery
snapshots, an offline build environment, deterministic acceptance checks, and
bounded retry/escalation behaviour.

## Recommended product pivot

Deliver a useful **Pinky Lite** milestone before resuming the complete version
one specification:

1. Preserve the encrypted vault, local ingestion, watching, lexical retrieval,
   and exact citation viewer already implemented.
2. Accept an explicitly configured loopback `llama-server`; do not initially
   build the full model catalogue and download manager.
3. Produce cited answers over retained local text using lexical evidence first.
   Qdrant and embeddings can remain dormant until verified artifacts exist.
4. Persist conversations and expose inference as cancellable tasks.
5. Add the offline development harness after useful cited Q&A works.

This avoids making vector retrieval, public-web research, image generation,
document extraction, and autonomous application generation prerequisites for
the first useful assistant workflow. Image metadata, bounded local OCR,
retained-pixel viewing, and image cancellation/restart acceptance are now
available. Image generation remains separate follow-up work. PDF or Office
extraction follows the prioritised image phase.

## Offline development loop

Each autonomous iteration must follow this sequence:

1. Select one bounded task whose expected output and acceptance checks fit in a
   short task packet.
2. Restore a clean task-specific workspace created from the last accepted
   checkpoint.
3. Provide only relevant source files, repository guidance, recent failures,
   and permitted tool schemas to the local model.
4. Create content-addressed recovery snapshots before edits.
5. Apply changes only within the approved repository root.
6. Build, test, and lint inside the declared rootless container or other
   approved sandbox.
7. Compare results with deterministic acceptance checks.
8. Retry at most twice with the new failure evidence.
9. Stop and request review after repeated failure, ambiguity, a permission
   expansion, or a security-sensitive decision.
10. Retain the diff, commands, logs, model identity, prompt, outcome, and
    rollback references in the encrypted task journal.

Passing tests allow a change to enter the review queue; they do not authorize
Pinky to commit, publish, permanently delete, disable controls, access secrets,
or expand its own permissions.

## Required offline kit

True offline operation requires these artifacts to be installed or cached
before disconnecting:

- pinned `llama-server` and Qdrant executables;
- verified GGUF chat/coding, embedding, and reranking models;
- Rust toolchain and required components;
- a vendored Cargo dependency tree for the lockfile;
- npm packages or an offline npm cache for every lockfile;
- rootless container images and their immutable digests;
- native build packages and extraction utilities;
- project documentation, fixtures, benchmarks, and model licences.

Budget approximately 40–100 GB for a useful initial bundle. A task that adds an
uncached package, model, image, or system dependency must enter
`waiting_for_user`; it cannot silently regain network access.

## Online and offline boundaries

Once provisioned, local conversation, ingestion, retrieval, generation, code
editing, builds, tests, and task history can remain offline. Network access is
still inherently required for public-web ingestion and research, and may be
needed deliberately for new dependencies, runtime updates, model artifacts,
or external review.

Expected development split for the self-building route:

| Milestone | Online or external effort | Offline local effort |
|---|---:|---:|
| Offline bootstrap and basic cited Q&A | 3–5 focused days | 1–3 weeks |
| Complete version one | 1–3 cumulative weeks | 10–20 weeks |

These are planning ranges, not delivery guarantees. They assume continuous
access to the target machine, bounded tasks, prepared artifacts, and periodic
human review. The approach reduces paid tokens by exchanging them for local
compute time, retries, and supervision.

## Acceptance gates

The self-development facility is not usable until automated tests demonstrate:

- no reads or writes outside the approved workspace and vault;
- network remains disabled except during an explicitly approved phase;
- secrets, host sockets, home directories, and inherited credentials are not
  exposed to workers;
- every pre-edit state can be restored without Git reset, checkout, or stash;
- cancellation leaves no child process or container running;
- repeated failure stops rather than looping indefinitely;
- task prompts and source content cannot alter permissions or budgets;
- a model cannot mark its own work accepted without independent checks;
- offline builds succeed from the prepared caches on a disconnected machine.

## Review policy

Use local automation for mechanical work and tightly specified features. Seek a
strong external review for changes involving vault cryptography, Secret
Service, path authorization, process supervision, container isolation, network
policy, migrations, deletion, backup/restore, or release acceptance. External
review should consume the smallest relevant diff and evidence bundle rather
than the entire conversation history.
