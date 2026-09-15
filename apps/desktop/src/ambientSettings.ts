import type { EntityState } from "./entity";
import { ENTITY_PALETTE } from "./entityMotion";

export type AmbientExpression = EntityState | "cycle";

export interface AmbientSettings {
  expression: AmbientExpression;
  cycleSeconds: number;
  scale: number;
  backgroundColor: string;
  backgroundOpacity: number;
  colors: {
    core: string;
    active: string;
    research: string;
    highlight: string;
    alert: string;
  };
  motion: {
    flow: number;
    reactions: number;
    glow: number;
  };
}

export const AMBIENT_STATES: EntityState[] = [
  "idle",
  "listening",
  "thinking",
  "researching",
  "creating",
  "waiting",
  "cancelling",
  "error",
  "completed",
];

export const DEFAULT_AMBIENT_SETTINGS: AmbientSettings = {
  expression: "idle",
  cycleSeconds: 8,
  scale: 1,
  backgroundColor: "#090a0d",
  backgroundOpacity: 0,
  colors: {
    core: "#e9a3bf",
    active: "#ffc2d6",
    research: "#7de2ff",
    highlight: "#ffc96b",
    alert: "#ff5268",
  },
  motion: {
    flow: 1,
    reactions: 1,
    glow: 1,
  },
};

const AMBIENT_SETTINGS_KEY = "pinky.ambient-alma.settings";
const HEX_COLOR = /^#[0-9a-f]{6}$/i;

const clamp = (value: number, minimum: number, maximum: number) => Math.min(maximum, Math.max(minimum, value));

function colorOrFallback(value: unknown, fallback: string): string {
  return typeof value === "string" && HEX_COLOR.test(value) ? value : fallback;
}

function numberOrFallback(value: unknown, fallback: number, minimum: number, maximum: number): number {
  return typeof value === "number" && Number.isFinite(value) ? clamp(value, minimum, maximum) : fallback;
}

export function cloneAmbientSettings(settings: AmbientSettings = DEFAULT_AMBIENT_SETTINGS): AmbientSettings {
  return {
    ...settings,
    colors: { ...settings.colors },
    motion: { ...settings.motion },
  };
}

export function normalizeAmbientSettings(candidate: unknown): AmbientSettings {
  const input = candidate && typeof candidate === "object" ? candidate as Partial<AmbientSettings> : {};
  const colors = input.colors && typeof input.colors === "object" ? input.colors as Partial<AmbientSettings["colors"]> : {};
  const motion = input.motion && typeof input.motion === "object" ? input.motion as Partial<AmbientSettings["motion"]> : {};
  const expression = input.expression === "cycle" || AMBIENT_STATES.includes(input.expression as EntityState)
    ? input.expression as AmbientExpression
    : DEFAULT_AMBIENT_SETTINGS.expression;

  return {
    expression,
    cycleSeconds: numberOrFallback(input.cycleSeconds, DEFAULT_AMBIENT_SETTINGS.cycleSeconds, 2, 30),
    scale: numberOrFallback(input.scale, DEFAULT_AMBIENT_SETTINGS.scale, 0.45, 1.7),
    backgroundColor: colorOrFallback(input.backgroundColor, DEFAULT_AMBIENT_SETTINGS.backgroundColor),
    backgroundOpacity: numberOrFallback(input.backgroundOpacity, DEFAULT_AMBIENT_SETTINGS.backgroundOpacity, 0, 0.85),
    colors: {
      core: colorOrFallback(colors.core, DEFAULT_AMBIENT_SETTINGS.colors.core),
      active: colorOrFallback(colors.active, DEFAULT_AMBIENT_SETTINGS.colors.active),
      research: colorOrFallback(colors.research, DEFAULT_AMBIENT_SETTINGS.colors.research),
      highlight: colorOrFallback(colors.highlight, DEFAULT_AMBIENT_SETTINGS.colors.highlight),
      alert: colorOrFallback(colors.alert, DEFAULT_AMBIENT_SETTINGS.colors.alert),
    },
    motion: {
      flow: numberOrFallback(motion.flow, DEFAULT_AMBIENT_SETTINGS.motion.flow, 0.1, 2),
      reactions: numberOrFallback(motion.reactions, DEFAULT_AMBIENT_SETTINGS.motion.reactions, 0, 2),
      glow: numberOrFallback(motion.glow, DEFAULT_AMBIENT_SETTINGS.motion.glow, 0.2, 2),
    },
  };
}

export function parseAmbientSettings(serialized: string | null): AmbientSettings {
  if (!serialized) return cloneAmbientSettings();
  try {
    return normalizeAmbientSettings(JSON.parse(serialized));
  } catch {
    return cloneAmbientSettings();
  }
}

export function loadAmbientSettings(): AmbientSettings {
  if (typeof localStorage === "undefined") return cloneAmbientSettings();
  return parseAmbientSettings(localStorage.getItem(AMBIENT_SETTINGS_KEY));
}

export function saveAmbientSettings(settings: AmbientSettings): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(AMBIENT_SETTINGS_KEY, JSON.stringify(normalizeAmbientSettings(settings)));
}

export function hexToNumber(color: string): number {
  return Number.parseInt(color.slice(1), 16);
}

export function ambientPalette(settings: AmbientSettings): Partial<Record<EntityState, number>> {
  return {
    ...ENTITY_PALETTE,
    idle: hexToNumber(settings.colors.core),
    waiting: hexToNumber(settings.colors.core),
    listening: hexToNumber(settings.colors.active),
    thinking: hexToNumber(settings.colors.active),
    researching: hexToNumber(settings.colors.research),
    creating: hexToNumber(settings.colors.highlight),
    completed: hexToNumber(settings.colors.highlight),
    cancelling: hexToNumber(settings.colors.alert),
    error: hexToNumber(settings.colors.alert),
  };
}

export { AMBIENT_SETTINGS_KEY };
