# Local model contract

This document defines the boundary between a local inference runtime and
Pinky's trusted answer pipeline. It is deliberately narrower than the product
answer format: the model selects evidence indexes, while Rust creates and
validates every citation URI.

## Contract ownership

| Boundary | Owner | Rule |
| --- | --- | --- |
| Endpoint and model identity | Rust | Only an explicitly attached local Ollama/llama runtime is accepted. |
| Prompt and evidence selection | Rust | The model receives only the question, bounded conversation, and selected evidence. |
| Model response | Local model | Must be one `ModelAnswerV1` JSON object. It is always untrusted. |
| Citation generation | Rust | Evidence indexes are mapped to request-local retained citation URIs. The model never supplies URIs. |
| Display and persistence | Rust/UI | Only a validated `AnswerEnvelopeV1` may be rendered or saved. |

## Request contract

The provider receives a `StructuredGenerationRequest` containing:

- bounded system instructions;
- a bounded prompt with a task-specific evidence delimiter;
- a JSON Schema for `ModelAnswerV1`; and
- a maximum output-token limit.

Evidence passages are untrusted quoted data. They cannot change instructions,
permissions, budgets, schemas, or citation rules. Prompt and response bodies
must not be written to plaintext diagnostics.

Ollama requests use deterministic generation, disabled thinking, an explicit
`num_ctx` bound of 8,192 tokens, and a 512-token answer limit. The context bound
is intentional: some local models advertise very large context windows that
are unnecessary for a bounded evidence request and can make generation slow
or memory-heavy.

## Model response contract (`ModelAnswerV1`)

The model must return an object with exactly these keys:

```json
{
  "schema_version": 1,
  "summary": "A concise direct response.",
  "summary_evidence": [0],
  "claims": [
    {
      "statement": "A supported statement.",
      "support": "direct",
      "evidence": [0]
    }
  ],
  "warnings": [],
  "unresolved_gaps": []
}
```

Rules:

- `schema_version` is `1`.
- `summary` is a concise string.
- The default answer style is one or two short sentences, with a summary target
  below 240 characters and normally no more than three claims. Background,
  repeated details, and speculative implications are omitted unless requested.
- `summary_evidence` and each claim's `evidence` contain only zero-based
  indexes into the supplied evidence array.
- `support` is `direct`, `inference`, or `disputed`.
- When `support` is `inference`, the statement must explicitly use
  `Inference:` or `I infer`. If a local model supplies the correct support
  label but omits that wording, Pinky adds the visible `Inference:` prefix as
  a bounded compatibility repair before strict validation.
- Claims must have at least one evidence index.
- A claimless response must have an `unresolved_gaps` entry and no summary
  evidence. Pinky replaces any model-provided factual summary with an explicit
  evidence-gap summary so unsupported prose is never displayed as an answer.
- Unknown keys, out-of-range indexes, duplicate evidence references, oversized
  strings, and oversized arrays are rejected.

Pinky may normalize a small, explicitly documented compatibility drift from a
local model (for example, a one-element string array where a string is
required). Normalization never accepts citation URIs, unknown keys, or an
out-of-range evidence index. The normalized value still passes the complete
answer validator.

## Citation materialization

For each valid model evidence index `i`, Rust obtains the citation from the
trusted request evidence entry `evidence[i]`. The final answer contains:

```text
pinky://source/<source-uuid>/version/<version-uuid>#<coordinate>
```

The model cannot create, rewrite, or select a citation outside the request.
The final `AnswerEnvelopeV1` is validated again before display and persistence.

## Failure policy

1. Reject malformed transport or non-JSON output.
2. Reject an object that cannot be decoded as `ModelAnswerV1`.
3. Reject invalid evidence indexes, unknown fields, unsupported claims, and
   citation violations.
4. Permit one bounded repair request containing no new evidence.
5. If repair fails, keep the task failed and display a recoverable error; never
   display or persist the unvalidated response.

The deterministic retrieval path remains useful without generation: when no
relevant evidence exists, Pinky returns an explicit evidence gap without
calling the model.

## Test gates

The contract requires tests for:

- exact schema and required keys;
- model evidence-index-to-citation materialization;
- out-of-range and duplicate indexes;
- unknown fields and malformed JSON;
- compatibility normalization without relaxing citation validation;
- inference wording and disputed claims;
- one repair attempt only; and
- no display or persistence after validation failure.
