import { describe, expect, it } from "vitest";
import { deriveEntityState } from "./entity";
describe("deriveEntityState", () => {
  it("reflects the highest-priority real task state", () => {
    expect(deriveEntityState([{ state: "running", phase: { name: "research web" } }])).toBe("researching");
    expect(deriveEntityState([{ state: "cancelling", phase: { name: "research" } }])).toBe("cancelling");
    expect(deriveEntityState([{ state: "failed", phase: { name: "ingest" } }])).toBe("error");
  });
});
