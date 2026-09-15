import { describe, expect, it } from "vitest";
import { ambientPalette, DEFAULT_AMBIENT_SETTINGS, normalizeAmbientSettings, parseAmbientSettings } from "./ambientSettings";

describe("ambient settings", () => {
  it("keeps malformed values inside the supported visual range", () => {
    const settings = normalizeAmbientSettings({
      expression: "not-a-state",
      scale: 99,
      backgroundOpacity: -1,
      colors: { core: "pink" },
      motion: { flow: 0, reactions: 4, glow: -2 },
    });

    expect(settings.expression).toBe("idle");
    expect(settings.scale).toBe(1.7);
    expect(settings.backgroundOpacity).toBe(0);
    expect(settings.colors.core).toBe(DEFAULT_AMBIENT_SETTINGS.colors.core);
    expect(settings.motion.flow).toBe(0.1);
    expect(settings.motion.reactions).toBe(2);
    expect(settings.motion.glow).toBe(0.2);
  });

  it("recovers from invalid persisted JSON", () => {
    expect(parseAmbientSettings("not json")).toEqual(DEFAULT_AMBIENT_SETTINGS);
  });

  it("maps the compact colour controls to every ALMA state", () => {
    const settings = normalizeAmbientSettings({ colors: { core: "#112233", alert: "#445566" } });
    const palette = ambientPalette(settings);

    expect(palette.idle).toBe(0x112233);
    expect(palette.waiting).toBe(0x112233);
    expect(palette.error).toBe(0x445566);
    expect(palette.cancelling).toBe(0x445566);
  });
});
