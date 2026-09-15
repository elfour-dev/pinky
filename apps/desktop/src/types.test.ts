import { describe, expect, it } from "vitest";
import { canAsk } from "./types";
import { isReconnectableError } from "./reliability";
import type { RuntimeStatus } from "./types";

const status: RuntimeStatus = {
  vault_mounted: true,
  setup_in_progress: false,
  vault_registered: true,
  vault_id: "vault",
  unlock_error: null,
  task_journal_error: null,
  watcher_error: null,
  model_attach_in_progress: false,
  model_connected: true,
  model_provider: "Ollama",
  model_name: "local",
  model_context_size: 4096,
  model_error: null,
  hybrid_configured: false,
  hybrid_model: null,
  prerequisites: {},
};

describe("Ask mode enablement", () => {
  it("requires an unlocked vault, attached model, source, and question", () => {
    expect(canAsk(status, "What does this say?", 1)).toBe(true);
    expect(canAsk({ ...status, vault_mounted: false }, "What does this say?", 1)).toBe(false);
    expect(canAsk({ ...status, model_connected: false }, "What does this say?", 1)).toBe(false);
    expect(canAsk(status, "What does this say?", 0)).toBe(false);
    expect(canAsk(status, "   ", 1)).toBe(false);
  });
});

describe("model reconnect errors", () => {
  it("recognizes transport failures without treating validation errors as reconnects", () => {
    expect(isReconnectableError("inference server is unavailable: tunnel closed")).toBe(true);
    expect(isReconnectableError("inference timed out")).toBe(true);
    expect(isReconnectableError("model answer remained invalid after one repair attempt")).toBe(false);
  });
});
