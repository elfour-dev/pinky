import { describe, expect, it } from "vitest";
import { canAsk } from "./types";
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
