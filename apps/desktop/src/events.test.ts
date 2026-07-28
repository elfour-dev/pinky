import { describe, expect, it } from "vitest";
import { mergeTaskEvents } from "./events";
import type { TaskEvent } from "./types";

function event(sequence: number, activity: string): TaskEvent {
  return {
    schema_major: 1,
    schema_minor: 0,
    sequence,
    task_id: "00000000-0000-0000-0000-000000000001",
    parent_id: null,
    timestamp: "2026-07-28T00:00:00Z",
    state: "running",
    phase: { name: "vault unlock", progress: null, activity },
    tool_name: null,
    resource_uri: null,
    permission_state: "approved",
    budget_state: "within_budget",
    cancellable: false,
    error: null,
  };
}

describe("mergeTaskEvents", () => {
  it("combines startup snapshots and live events without duplicates", () => {
    const merged = mergeTaskEvents([event(3, "live")], [event(2, "snapshot"), event(3, "live")]);
    expect(merged.map(({ sequence }) => sequence)).toEqual([2, 3]);
  });

  it("uses the newest payload for an existing sequence", () => {
    expect(mergeTaskEvents([event(4, "old")], [event(4, "updated")])[0].phase.activity).toBe("updated");
  });
});
