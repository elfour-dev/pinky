import { describe, expect, it } from "vitest";
import { sampleEntityMotion } from "./entityMotion";

describe("sampleEntityMotion", () => {
  it("gives idle a slow breathing cycle", () => {
    const first = sampleEntityMotion("idle", 0, false);
    const quarter = sampleEntityMotion("idle", Math.PI / 2 / 1.4, false);

    expect(first.rotationY).toBe(0);
    expect(quarter.scale).toBeGreaterThan(first.scale);
    expect(quarter.scale - first.scale).toBeLessThan(0.08);
  });

  it("uses distinct task-state signals", () => {
    const thinking = sampleEntityMotion("thinking", 0.5, false);
    const thinkingContracted = sampleEntityMotion("thinking", (Math.PI * 1.5) / 2.8, false);
    const research = sampleEntityMotion("researching", 0.5, false);
    const researchExpanded = sampleEntityMotion("researching", Math.PI / (2 * 2.2), false);
    const researchFading = sampleEntityMotion("researching", 2.4 * 0.65, false);
    const researchAtEdge = sampleEntityMotion("researching", 2.4 * 0.9, false);
    const creating = sampleEntityMotion("creating", 0, false);
    const waiting = sampleEntityMotion("waiting", 0.5, false);

    expect(Math.abs(thinking.rotationY)).toBeGreaterThan(0.2);
    expect(thinking.jitter).toBeGreaterThan(0);
    expect(thinkingContracted.radialScale).toBeLessThan(0.8);
    expect(researchExpanded.radialScale).toBeGreaterThan(1.08);
    expect(research.ringOpacity).toBeGreaterThan(0);
    expect(researchFading.ringOpacity).toBeLessThan(1);
    expect(researchAtEdge.ringOpacity).toBe(0);
    expect(creating.radialScale).toBeLessThan(1);
    expect(waiting.waitingOpacity).toBe(0.95);
  });

  it("keeps the face within a forward-facing turn window", () => {
    const states = ["idle", "listening", "thinking", "researching", "creating", "waiting", "cancelling", "error", "completed"] as const;

    for (const state of states) {
      for (const elapsed of [0, 1, 10, 100]) {
        expect(Math.abs(sampleEntityMotion(state, elapsed, false).rotationY)).toBeLessThan(0.5);
      }
    }
  });

  it("contracts cancellation and fractures errors", () => {
    const early = sampleEntityMotion("cancelling", 0, false);
    const late = sampleEntityMotion("cancelling", 3, false);

    expect(late.scale).toBeLessThan(early.scale);
    expect(early.fracture).toBeGreaterThan(0);
    expect(sampleEntityMotion("error", 30, false).rotationY).toBe(0);
  });

  it("makes completion a brief expansion before settling", () => {
    const burst = sampleEntityMotion("completed", 0, false);
    const settled = sampleEntityMotion("completed", 2, false);

    expect(burst.scale).toBeGreaterThan(1);
    expect(settled.scale).toBeCloseTo(1, 1);
  });

  it("removes continuous motion when reduced motion is requested", () => {
    const sample = sampleEntityMotion("researching", 12, true);

    expect(sample.rotationY).toBe(0);
    expect(sample.ringOpacity).toBe(0);
    expect(sample.jitter).toBe(0);
  });
});
