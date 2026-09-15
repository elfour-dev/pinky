import type { EntityState } from "./entity";

export interface EntityMotionSample {
  rotationY: number;
  rotationX: number;
  rotationZ: number;
  scale: number;
  radialScale: number;
  jitter: number;
  ringProgress: number;
  ringOpacity: number;
  opacity: number;
  pointSize: number;
  fracture: number;
  waitingOpacity: number;
}

const easeOutCubic = (value: number) => 1 - Math.pow(1 - Math.max(0, Math.min(1, value)), 3);

/**
 * Samples ALMA's visual state from elapsed time. Keeping this separate from
 * Three.js makes the motion contract deterministic and easy to exercise in
 * tests, while the renderer remains responsible only for drawing the sample.
 */
export function sampleEntityMotion(state: EntityState, elapsedSeconds: number, reducedMotion: boolean): EntityMotionSample {
  const elapsed = Math.max(0, elapsedSeconds);

  if (reducedMotion) {
    return {
      rotationY: state === "listening" ? 0.22 : 0,
      rotationX: 0,
      rotationZ: 0,
      scale: 1,
      radialScale: 1,
      jitter: 0,
      ringProgress: 0,
      ringOpacity: 0,
      opacity: state === "cancelling" ? 0.55 : state === "waiting" ? 0.32 : 0.9,
      pointSize: state === "thinking" ? 0.029 : 0.025,
      fracture: state === "error" ? 1 : state === "cancelling" ? 0.5 : 0,
      waitingOpacity: state === "waiting" ? 0.95 : 0,
    };
  }

  switch (state) {
    case "idle": {
      const breath = Math.sin(elapsed * 1.4);
      return {
        rotationY: Math.sin(elapsed * 0.32) * 0.24,
        rotationX: Math.sin(elapsed * 0.33) * 0.14,
        rotationZ: Math.sin(elapsed * 0.23) * 0.04,
        scale: 1 + breath * 0.055,
        radialScale: 1,
        jitter: 0,
        ringProgress: 0,
        ringOpacity: 0,
        opacity: 0.9,
        pointSize: 0.025,
        fracture: 0,
        waitingOpacity: 0,
      };
    }
    case "listening":
      return {
        rotationY: 0.24 + Math.sin(elapsed * 0.8) * 0.12,
        rotationX: Math.sin(elapsed * 0.65) * 0.08,
        rotationZ: Math.sin(elapsed * 0.9) * 0.06,
        scale: 1 + Math.sin(elapsed * 2) * 0.02,
        radialScale: 1,
        jitter: 0,
        ringProgress: 0,
        ringOpacity: 0,
        opacity: 0.94,
        pointSize: 0.03,
        fracture: 0,
        waitingOpacity: 0,
      };
    case "thinking": {
      const inward = Math.sin(elapsed * 2.8);
      return {
        rotationY: Math.sin(elapsed * 2.6) * 0.34,
        rotationX: Math.sin(elapsed * 1.2) * 0.2,
        rotationZ: Math.sin(elapsed * 1.6) * 0.05,
        scale: 1 + Math.sin(elapsed * 3.8) * 0.11,
        radialScale: 0.9 + inward * 0.12,
        jitter: 0.022,
        ringProgress: 0,
        ringOpacity: 0,
        opacity: 0.92,
        pointSize: 0.029,
        fracture: 0,
        waitingOpacity: 0,
      };
    }
    case "researching": {
      const ringProgress = (elapsed % 2.4) / 2.4;
      const fadeStart = 0.46;
      const fadeEnd = 0.8;
      const fadeAmount = Math.max(0, Math.min(1, (ringProgress - fadeStart) / (fadeEnd - fadeStart)));
      const ringOpacity = 1 - (fadeAmount * fadeAmount * (3 - 2 * fadeAmount));
      return {
        rotationY: Math.sin(elapsed * 1.4) * 0.28,
        rotationX: Math.sin(elapsed * 0.55) * 0.12,
        rotationZ: 0,
        scale: 1 + Math.sin(elapsed * 1.8) * 0.08,
        radialScale: 1 + Math.sin(elapsed * 2.2) * 0.09,
        jitter: 0.012,
        ringProgress,
        ringOpacity,
        opacity: 0.9,
        pointSize: 0.025,
        fracture: 0,
        waitingOpacity: 0,
      };
    }
    case "creating": {
      const assembly = easeOutCubic(elapsed / 1.8);
      return {
        rotationY: Math.sin(elapsed * 0.65) * 0.22,
        rotationX: Math.sin(elapsed * 0.7) * 0.08,
        rotationZ: Math.sin(elapsed * 1.1) * 0.05,
        scale: 1 + Math.sin(elapsed * 2.8) * 0.04,
        radialScale: 0.28 + assembly * 0.72,
        jitter: (1 - assembly) * 0.055,
        ringProgress: 0,
        ringOpacity: 0,
        opacity: 0.92,
        pointSize: 0.027,
        fracture: 0,
        waitingOpacity: 0,
      };
    }
    case "waiting":
      return {
        rotationY: Math.sin(elapsed * 0.25) * 0.015,
        rotationX: 0,
        rotationZ: 0,
        scale: 1 + Math.sin(elapsed * 0.4) * 0.005,
        radialScale: 1,
        jitter: 0,
        ringProgress: 0,
        ringOpacity: 0,
        opacity: 0.28,
        pointSize: 0.024,
        fracture: 0,
        waitingOpacity: 0.95,
      };
    case "cancelling": {
      const contraction = Math.min(elapsed / 2, 1);
      return {
        rotationY: Math.sin(elapsed * 0.05) * 0.05,
        rotationX: Math.sin(elapsed * 0.3) * 0.02,
        rotationZ: 0,
        scale: 1 - contraction * 0.12,
        radialScale: 1 - contraction * 0.08,
        jitter: 0,
        ringProgress: 0,
        ringOpacity: 0,
        opacity: 0.72 - contraction * 0.2,
        pointSize: 0.022,
        fracture: 0.5,
        waitingOpacity: 0,
      };
    }
    case "error":
      return {
        rotationY: 0,
        rotationX: 0,
        rotationZ: 0,
        scale: 1,
        radialScale: 1,
        jitter: 0,
        ringProgress: 0,
        ringOpacity: 0,
        opacity: 0.92,
        pointSize: 0.026,
        fracture: 1,
        waitingOpacity: 0,
      };
    case "completed": {
      const burst = Math.max(0, 1 - elapsed / 1.2);
      const settlingBreath = Math.sin(Math.max(0, elapsed - 1.2) * 1.4) * 0.028;
      return {
        rotationY: Math.sin(elapsed * 0.25) * 0.18,
        rotationX: Math.sin(elapsed * 0.35) * 0.07,
        rotationZ: 0,
        scale: 1 + easeOutCubic(burst) * 0.18 + (burst === 0 ? settlingBreath : 0),
        radialScale: 1,
        jitter: 0,
        ringProgress: 0,
        ringOpacity: 0,
        opacity: 0.92,
        pointSize: 0.027,
        fracture: 0,
        waitingOpacity: 0,
      };
    }
  }
}

export const ENTITY_PALETTE: Record<EntityState, number> = {
  idle: 0xe9a3bf,
  listening: 0xffc2d6,
  thinking: 0xc89cff,
  researching: 0x7de2ff,
  creating: 0xffc96b,
  waiting: 0xf4d97b,
  cancelling: 0x887681,
  error: 0xff5268,
  completed: 0x8af0bb,
};
