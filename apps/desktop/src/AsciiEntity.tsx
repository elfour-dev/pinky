import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import type { EntityState } from "./entity";
import { ENTITY_PALETTE, sampleEntityMotion } from "./entityMotion";

const FALLBACK_ART: Record<EntityState, string[]> = {
  idle: ["   .-~~-. ", " /   ::   \\", "|   ~~~~   |", " \\  ____  /", "  '-.__.-'"],
  listening: [" · .-~~-. · ", " /   ::   \\", "|  ·····  |", " \\  ____  /", "  '-.__.-'"],
  thinking: ["   .-~~-. ", " /   ≋≋   \\", "|  ≋≋≋≋  |", " \\  ____  /", "  '-.__.-'"],
  researching: ["   .-~~-. ", " /   ◌◌   \\", "|  (~~~)  |", " \\  ____  /", "  '-.__.-'"],
  creating: ["   .-~~-. ", " /   ++   \\", "|  .-=-.  |", " \\  ____  /", "  '-.__.-'"],
  waiting: ["   .-~~-. ", " /   ..   \\", "|   ···   |", " \\  ____  /", "  '-.__.-'"],
  cancelling: ["   .-~~-. ", " /   --   \\", "|    ..   |", " \\  ____  /", "  '-.__.-'"],
  error: ["   .-//-. ", " /   !!   \\", "|  /##\\  |", " \\  ____  /", "  '-.__.-'"],
  completed: ["   .-**-. ", " /   ++   \\", "|    **   |", " \\  ____  /", "  '-.__.-'"],
};

const FLUIDITY: Record<EntityState, number> = {
  idle: 0.55,
  listening: 0.35,
  thinking: 0.72,
  researching: 0.62,
  creating: 0.95,
  waiting: 0.06,
  cancelling: 0.22,
  error: 0.02,
  completed: 0.42,
};

const SHAPE_AGGRESSION: Record<EntityState, number> = {
  idle: 0.18,
  listening: 0.12,
  thinking: 1,
  researching: 0.86,
  creating: 0.62,
  waiting: 0.03,
  cancelling: 0.2,
  error: 0.01,
  completed: 0.2,
};

const REACTION_INTENSITY: Record<EntityState, number> = {
  idle: 0,
  listening: 0,
  thinking: 1,
  researching: 0.92,
  creating: 0.28,
  waiting: 0,
  cancelling: 0.08,
  error: 0,
  completed: 0,
};

const REACTION_RATE: Record<EntityState, number> = {
  idle: 2.8,
  listening: 2.8,
  thinking: 4.8,
  researching: 3.6,
  creating: 3.2,
  waiting: 2.2,
  cancelling: 2.2,
  error: 1.2,
  completed: 2.8,
};

const lerp = (from: number, to: number, amount: number) => from + (to - from) * amount;
const smoothStep = (amount: number) => {
  const clamped = Math.max(0, Math.min(1, amount));
  return clamped * clamped * (3 - 2 * clamped);
};

export interface EntityVisualConfig {
  palette?: Partial<Record<EntityState, number>>;
  motion?: {
    flow?: number;
    reactions?: number;
    glow?: number;
  };
}

function blendMotion(from: ReturnType<typeof sampleEntityMotion>, to: ReturnType<typeof sampleEntityMotion>, amount: number) {
  return {
    rotationY: lerp(from.rotationY, to.rotationY, amount),
    rotationX: lerp(from.rotationX, to.rotationX, amount),
    rotationZ: lerp(from.rotationZ, to.rotationZ, amount),
    scale: lerp(from.scale, to.scale, amount),
    radialScale: lerp(from.radialScale, to.radialScale, amount),
    jitter: lerp(from.jitter, to.jitter, amount),
    ringProgress: lerp(from.ringProgress, to.ringProgress, amount),
    ringOpacity: lerp(from.ringOpacity, to.ringOpacity, amount),
    opacity: lerp(from.opacity, to.opacity, amount),
    pointSize: lerp(from.pointSize, to.pointSize, amount),
    fracture: lerp(from.fracture, to.fracture, amount),
    waitingOpacity: lerp(from.waitingOpacity, to.waitingOpacity, amount),
  };
}

function makeWaitingMoteGeometry() {
  const positions: number[] = [];
  for (const x of [-0.28, 0, 0.28]) positions.push(x, -0.05, 1.4);
  return new THREE.Float32BufferAttribute(positions, 3);
}

function spriteRandom(seed: number) {
  let value = Math.imul(seed ^ 61, 0x45d9f3b);
  value = Math.imul(value ^ (value >>> 16), 0x45d9f3b);
  value ^= value >>> 16;
  return (value >>> 0) / 4_294_967_296;
}

function makeSparkGeometry(count: number) {
  const positions: number[] = [];
  for (let index = 0; index < count; index += 1) {
    const phi = Math.acos(1 - (2 * (index + 0.5)) / count);
    const theta = Math.PI * (1 + Math.sqrt(5)) * index;
    const radius = 1.65 + spriteRandom(index + 1201) * 0.65;
    positions.push(radius * Math.cos(theta) * Math.sin(phi), radius * Math.sin(theta) * Math.sin(phi), radius * Math.cos(phi));
  }
  return new THREE.Float32BufferAttribute(positions, 3);
}

export function AsciiEntity({ state, reducedMotion, palette, motion }: { state: EntityState; reducedMotion: boolean } & EntityVisualConfig) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const fallback = typeof WebGLRenderingContext === "undefined";
  const [webglUnavailable, setWebglUnavailable] = useState(false);
  const stateRef = useRef(state);
  const previousStateRef = useRef(state);
  const reducedMotionRef = useRef(reducedMotion);
  const stateStartedAtRef = useRef(0);
  const transitionStartedAtRef = useRef(0);
  const transitionFromElapsedRef = useRef(0);
  const visualRef = useRef({
    palette: { ...ENTITY_PALETTE, ...palette },
    motion: { flow: 1, reactions: 1, glow: 1, ...motion },
    customized: palette !== undefined || motion !== undefined,
  });

  useEffect(() => {
    const now = performance.now();
    if (stateStartedAtRef.current === 0) {
      stateStartedAtRef.current = now;
      transitionStartedAtRef.current = now;
    } else if (stateRef.current !== state) {
      previousStateRef.current = stateRef.current;
      transitionFromElapsedRef.current = Math.max(0, (now - stateStartedAtRef.current) / 1000);
      transitionStartedAtRef.current = now;
      stateStartedAtRef.current = now;
    }
    stateRef.current = state;
  }, [state]);

  useEffect(() => {
    reducedMotionRef.current = reducedMotion;
  }, [reducedMotion]);

  useEffect(() => {
    visualRef.current = {
      palette: { ...ENTITY_PALETTE, ...palette },
      motion: { flow: 1, reactions: 1, glow: 1, ...motion },
      customized: palette !== undefined || motion !== undefined,
    };
  }, [motion, palette]);

  useEffect(() => {
    if (fallback || webglUnavailable || !canvasRef.current) return;

    const canvas = canvasRef.current;
    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true });
    } catch {
      setWebglUnavailable(true);
      return;
    }
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.5));
    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(48, 1, 0.1, 100);
    camera.position.z = 5;

    const pointCount = 1100;
    const basePositions = new Float32Array(pointCount * 3);
    for (let index = 0; index < pointCount; index += 1) {
      const phi = Math.acos(1 - (2 * (index + 0.5)) / pointCount);
      const theta = Math.PI * (1 + Math.sqrt(5)) * index;
      const radius = 1.45 + 0.025 * Math.sin(theta * 2) * Math.sin(phi);
      const offset = index * 3;
      basePositions[offset] = radius * Math.cos(theta) * Math.sin(phi);
      basePositions[offset + 1] = radius * Math.sin(theta) * Math.sin(phi);
      basePositions[offset + 2] = radius * Math.cos(phi);
    }

    const geometry = new THREE.BufferGeometry();
    const positionAttribute = new THREE.Float32BufferAttribute(new Float32Array(basePositions), 3);
    geometry.setAttribute("position", positionAttribute);
    const initialPalette = visualRef.current.palette;
    const material = new THREE.PointsMaterial({ color: initialPalette[state], size: 0.025, transparent: true, opacity: 0.9 });
    const cloud = new THREE.Points(geometry, material);
    scene.add(cloud);

    const pulseGeometry = new THREE.BufferGeometry();
    const pulsePositionAttribute = new THREE.Float32BufferAttribute(new Float32Array(basePositions), 3);
    const pulsePhases = new Float32Array(pointCount);
    const pulsePeriods = new Float32Array(pointCount);
    const pulseStrengths = new Float32Array(pointCount);
    const pulseSizes = new Float32Array(pointCount);
    const listeningDistances = new Float32Array(pointCount);
    const listeningStrengths = new Float32Array(pointCount);
    for (let index = 0; index < pointCount; index += 1) {
      // Use independent seeded values so Fibonacci point ordering cannot form a
      // visible travelling ring while keeping the visual reproducible.
      const selection = spriteRandom(index + 11);
      const offset = index * 3;
      const radius = Math.max(
        Math.hypot(basePositions[offset], basePositions[offset + 1], basePositions[offset + 2]),
        0.001,
      );
      const surfaceNormalX = basePositions[offset] / radius;
      pulsePhases[index] = spriteRandom(index + 101);
      pulsePeriods[index] = 3.4 + spriteRandom(index + 211) * 5.2;
      pulseStrengths[index] = selection > 0.9 ? 0.7 + spriteRandom(index + 307) * 0.3 : 0;
      pulseSizes[index] = 0.8 + spriteRandom(index + 401) * 0.5;
      listeningDistances[index] = (surfaceNormalX + 1) * 0.5;
      listeningStrengths[index] = 0.55 + spriteRandom(index + 501) * 0.45;
    }
    pulseGeometry.setAttribute("position", pulsePositionAttribute);
    pulseGeometry.setAttribute("aPulse", new THREE.Float32BufferAttribute(pulseStrengths, 1));
    pulseGeometry.setAttribute("aPhase", new THREE.Float32BufferAttribute(pulsePhases, 1));
    pulseGeometry.setAttribute("aPeriod", new THREE.Float32BufferAttribute(pulsePeriods, 1));
    pulseGeometry.setAttribute("aSize", new THREE.Float32BufferAttribute(pulseSizes, 1));
    pulseGeometry.setAttribute("aListenDistance", new THREE.Float32BufferAttribute(listeningDistances, 1));
    pulseGeometry.setAttribute("aListenStrength", new THREE.Float32BufferAttribute(listeningStrengths, 1));
    const pulseMaterial = new THREE.ShaderMaterial({
      transparent: true,
      depthWrite: false,
      depthTest: false,
      blending: THREE.AdditiveBlending,
      uniforms: {
        uTime: { value: 0 },
        uOpacity: { value: 0.7 },
        uListeningTime: { value: 0 },
        uListeningPresence: { value: 0 },
        uColor: { value: new THREE.Color(initialPalette[state]) },
      },
      vertexShader: `
        attribute float aPulse;
        attribute float aPhase;
        attribute float aPeriod;
        attribute float aSize;
        attribute float aListenDistance;
        attribute float aListenStrength;
        uniform float uTime;
        uniform float uListeningTime;
        uniform float uListeningPresence;
        varying float vPulse;
        void main() {
          float cycle = fract(uTime / aPeriod + aPhase);
          float burst = smoothstep(0.0, 0.08, cycle) * (1.0 - smoothstep(0.08, 0.24, cycle));
          float listeningProgress = fract(uListeningTime / 2.4);
          float leftFront = 1.0 - smoothstep(0.0, 0.085, abs(aListenDistance - listeningProgress));
          float rightFront = 1.0 - smoothstep(0.0, 0.085, abs((1.0 - aListenDistance) - listeningProgress));
          float leftPulse = leftFront * aListenStrength * uListeningPresence;
          float rightPulse = rightFront * aListenStrength * uListeningPresence;
          float listeningPulse = max(leftPulse, rightPulse);
          float crossingPulse = leftFront * rightFront * aListenStrength * uListeningPresence;
          vec3 surfaceNormal = normalize(position);
          vec3 rightTravelAxis = vec3(1.0, 0.0, 0.0);
          vec3 rightTangent = rightTravelAxis - dot(rightTravelAxis, surfaceNormal) * surfaceNormal;
          rightTangent /= max(length(rightTangent), 0.001);
          float rightPulseDominant = step(leftPulse, rightPulse);
          vec3 directionalPulse = mix(rightTangent * leftPulse, -rightTangent * rightPulse, rightPulseDominant);
          vec3 pulsePosition = position + directionalPulse * (0.035 + listeningProgress * 0.11);
          float backgroundPulsePresence = 1.0 - uListeningPresence;
          vPulse = aPulse * (0.16 + burst * 0.95) * backgroundPulsePresence + listeningPulse * 0.95 + crossingPulse * 0.7;
          vec4 modelPosition = modelViewMatrix * vec4(pulsePosition, 1.0);
          float pointScale = aPulse * (1.6 + burst * 4.2) * backgroundPulsePresence + listeningPulse * 4.8 + crossingPulse * 2.2;
          gl_PointSize = aSize * 0.025 * pointScale * (300.0 / max(-modelPosition.z, 1.0));
          gl_Position = projectionMatrix * modelPosition;
        }
      `,
      fragmentShader: `
        uniform float uOpacity;
        uniform vec3 uColor;
        varying float vPulse;
        void main() {
          float distanceFromCentre = distance(gl_PointCoord, vec2(0.5));
          float softness = 1.0 - smoothstep(0.18, 0.5, distanceFromCentre);
          float alpha = softness * vPulse * uOpacity;
          if (alpha < 0.01) discard;
          gl_FragColor = vec4(uColor, alpha);
        }
      `,
    });
    const pulseCloud = new THREE.Points(pulseGeometry, pulseMaterial);
    cloud.add(pulseCloud);

    const listeningSourceGeometry = new THREE.BufferGeometry();
    listeningSourceGeometry.setAttribute(
      "position",
      new THREE.Float32BufferAttribute([-1.62, 0, 0.1, 1.62, 0, 0.1], 3),
    );
    const listeningSourceMaterial = new THREE.PointsMaterial({
      color: initialPalette.listening,
      size: 0.08,
      transparent: true,
      opacity: 0,
      depthWrite: false,
      depthTest: false,
    });
    const listeningSources = new THREE.Points(listeningSourceGeometry, listeningSourceMaterial);
    cloud.add(listeningSources);

    const sparkGeometry = new THREE.BufferGeometry();
    sparkGeometry.setAttribute("position", makeSparkGeometry(64));
    const sparkMaterial = new THREE.PointsMaterial({ color: initialPalette.creating, size: 0.035, transparent: true, opacity: 0, depthWrite: false, depthTest: false });
    const sparks = new THREE.Points(sparkGeometry, sparkMaterial);
    cloud.add(sparks);

    const ringGeometry = new THREE.RingGeometry(1.62, 1.65, 96);
    const ringMaterial = new THREE.MeshBasicMaterial({ color: initialPalette.researching, transparent: true, opacity: 0, side: THREE.DoubleSide });
    const ring = new THREE.Mesh(ringGeometry, ringMaterial);
    scene.add(ring);

    const waitingMoteGeometry = new THREE.BufferGeometry();
    waitingMoteGeometry.setAttribute("position", makeWaitingMoteGeometry());
    const waitingMoteMaterial = new THREE.PointsMaterial({ color: initialPalette.waiting, size: 0.04, transparent: true, opacity: 0, depthWrite: false });
    const waitingMotes = new THREE.Points(waitingMoteGeometry, waitingMoteMaterial);
    cloud.add(waitingMotes);

    const colors = Object.fromEntries(Object.keys(ENTITY_PALETTE).map((key) => {
      const stateKey = key as EntityState;
      return [stateKey, new THREE.Color(initialPalette[stateKey])];
    })) as Record<EntityState, THREE.Color>;
    const updateColors = () => {
      const currentPalette = visualRef.current.palette;
      for (const stateKey of Object.keys(ENTITY_PALETTE) as EntityState[]) colors[stateKey].set(currentPalette[stateKey]);
    };
    let animation = 0;
    let lastFrameAt = performance.now();
    const mountedAt = lastFrameAt;
    let renderedWidth = 0;
    let renderedHeight = 0;
    const transitionDuration = 0.9;

    const render = () => {
      const now = performance.now();
      const delta = Math.min(Math.max((now - lastFrameAt) / 1000, 0), 0.1);
      lastFrameAt = now;
      const currentState = stateRef.current;
      const previousState = previousStateRef.current;
      const visual = visualRef.current.motion;
      const transitionAmount = smoothStep((now - transitionStartedAtRef.current) / (transitionDuration * 1000));
      const currentElapsed = Math.max(0, (now - stateStartedAtRef.current) / 1000) * (visual.flow ?? 1);
      const previousElapsed = (transitionFromElapsedRef.current + Math.max(0, (now - transitionStartedAtRef.current) / 1000)) * (visual.flow ?? 1);
      const sample = blendMotion(
        sampleEntityMotion(previousState, previousElapsed, reducedMotionRef.current),
        sampleEntityMotion(currentState, currentElapsed, reducedMotionRef.current),
        transitionAmount,
      );
      const flowTime = (now - mountedAt) / 1000 * (visual.flow ?? 1);
      const fluidity = reducedMotionRef.current ? 0 : lerp(FLUIDITY[previousState], FLUIDITY[currentState], transitionAmount);
      const shapeAggression = reducedMotionRef.current ? 0 : lerp(SHAPE_AGGRESSION[previousState], SHAPE_AGGRESSION[currentState], transitionAmount);
      const reactionIntensity = reducedMotionRef.current ? 0 : lerp(REACTION_INTENSITY[previousState], REACTION_INTENSITY[currentState], transitionAmount) * (visual.reactions ?? 1);
      const reactionRate = lerp(REACTION_RATE[previousState], REACTION_RATE[currentState], transitionAmount);
      const reactionEnvelope = 0.55 + 0.45 * Math.sin(flowTime * reactionRate);
      const listeningPresence = lerp(previousState === "listening" ? 1 : 0, currentState === "listening" ? 1 : 0, transitionAmount);
      const creatingPresence = lerp(previousState === "creating" ? 1 : 0, currentState === "creating" ? 1 : 0, transitionAmount);
      const completedPresence = lerp(previousState === "completed" ? 1 : 0, currentState === "completed" ? 1 : 0, transitionAmount);

      const rect = canvas.getBoundingClientRect();
      const width = Math.max(1, rect.width);
      const height = Math.max(1, rect.height);
      if (Math.abs(width - renderedWidth) > 0.5 || Math.abs(height - renderedHeight) > 0.5) {
        renderedWidth = width;
        renderedHeight = height;
        renderer.setSize(width, height, false);
        camera.aspect = width / height;
        camera.updateProjectionMatrix();
      }

      const positions = positionAttribute.array as Float32Array;
      const pulsePositions = pulsePositionAttribute.array as Float32Array;
      const pointFollow = 1 - Math.exp(-delta * 12);
      for (let index = 0; index < pointCount; index += 1) {
        const offset = index * 3;
        const x = basePositions[offset];
        const y = basePositions[offset + 1];
        const z = basePositions[offset + 2];
        const baseRadius = Math.max(Math.sqrt(x * x + y * y + z * z), 0.001);
        const normalX = x / baseRadius;
        const normalY = y / baseRadius;
        const normalZ = z / baseRadius;
        const rawFlowX = Math.sin(normalY * 2.7 + normalZ * 1.8 + flowTime * 0.9);
        const rawFlowY = Math.sin(normalZ * 2.3 - normalX * 2.1 + flowTime * 0.72);
        const rawFlowZ = Math.sin(normalX * 2.5 - normalY * 1.9 - flowTime * 0.82);
        const flowProjection = rawFlowX * normalX + rawFlowY * normalY + rawFlowZ * normalZ;
        const tangentX = rawFlowX - flowProjection * normalX;
        const tangentY = rawFlowY - flowProjection * normalY;
        const tangentZ = rawFlowZ - flowProjection * normalZ;
        const radialRipple = Math.sin(normalX * 2.1 + normalY * 2.8 + normalZ * 1.7 + flowTime * 0.65);
        const aggressiveRipple = Math.sin(normalX * 4.2 + normalY * 2.7 + flowTime * 2.2) * Math.sin(normalZ * 3.1 - flowTime * 1.7);
        const pointNoise = spriteRandom(index + 907) * 2 - 1;
        const reactionX = Math.sin(normalY * 8.7 + normalZ * 5.4 + flowTime * 3.8 + pointNoise * 6.2);
        const reactionY = Math.sin(normalZ * 9.1 - normalX * 6.6 - flowTime * 4.4 + pointNoise * 4.7);
        const reactionZ = Math.sin(normalX * 7.8 - normalY * 8.3 + flowTime * 4.1 + pointNoise * 5.5);
        const reactionProjection = reactionX * normalX + reactionY * normalY + reactionZ * normalZ;
        const reactionTangentX = reactionX - reactionProjection * normalX;
        const reactionTangentY = reactionY - reactionProjection * normalY;
        const reactionTangentZ = reactionZ - reactionProjection * normalZ;
        const reactionRipple = Math.sin(normalX * 7.4 + normalY * 6.1 - normalZ * 5.8 + flowTime * 4.7 + pointNoise * 3.5);
        const surfaceScale = sample.radialScale * (1 + radialRipple * fluidity * 0.028 + aggressiveRipple * shapeAggression * 0.075);
        const flowGain = fluidity * (0.055 + shapeAggression * 0.06);
        const reactionGain = reactionIntensity * reactionEnvelope * 0.13;
        const fractureBreak = sample.fracture * ((index % 17 === 0 || index % 29 === 0) ? 0.2 : 0.025);
        const jitterX = Math.sin(index * 12.9898) * sample.jitter;
        const jitterY = Math.cos(index * 78.233) * sample.jitter;
        const jitterZ = Math.sin(index * 39.425) * sample.jitter;
        const reactionScale = surfaceScale + reactionRipple * reactionGain;
        const nextX = x * (reactionScale + fractureBreak) + tangentX * flowGain + reactionTangentX * reactionGain + jitterX;
        const nextY = y * (reactionScale - fractureBreak * 0.35) + tangentY * flowGain + reactionTangentY * reactionGain + jitterY;
        const nextZ = z * (reactionScale + fractureBreak * 0.5) + tangentZ * flowGain + reactionTangentZ * reactionGain + jitterZ;
        positions[offset] = lerp(positions[offset], nextX, pointFollow);
        positions[offset + 1] = lerp(positions[offset + 1], nextY, pointFollow);
        positions[offset + 2] = lerp(positions[offset + 2], nextZ, pointFollow);
        pulsePositions[offset] = positions[offset];
        pulsePositions[offset + 1] = positions[offset + 1];
        pulsePositions[offset + 2] = positions[offset + 2];
      }
      positionAttribute.needsUpdate = true;
      pulsePositionAttribute.needsUpdate = true;
      if (visualRef.current.customized) updateColors();

      cloud.rotation.y = sample.rotationY;
      cloud.rotation.x = sample.rotationX;
      cloud.rotation.z = sample.rotationZ;
      cloud.scale.setScalar(sample.scale);
      material.color.lerp(colors[currentState], Math.min(1, delta * 8));
      const glow = visual.glow ?? 1;
      material.opacity = Math.min(1, sample.opacity * glow);
      material.size = sample.pointSize;
      pulseMaterial.uniforms.uTime.value = (now - mountedAt) / 1000;
      pulseMaterial.uniforms.uOpacity.value = reducedMotionRef.current ? 0 : Math.min(1, Math.max(0.25, sample.opacity * 0.8) * glow);
      pulseMaterial.uniforms.uListeningTime.value = flowTime;
      pulseMaterial.uniforms.uListeningPresence.value = reducedMotionRef.current ? 0 : listeningPresence;
      (pulseMaterial.uniforms.uColor.value as THREE.Color).lerp(colors[currentState], Math.min(1, delta * 6));

      const listeningOpacity = Math.min(1, listeningPresence * 0.88 * glow);
      listeningSources.visible = listeningPresence > 0.001;
      listeningSourceMaterial.color.lerp(colors.listening, Math.min(1, delta * 6));
      listeningSourceMaterial.opacity = listeningOpacity;
      listeningSourceMaterial.size = Math.max(0.065, sample.pointSize * 3);

      const completionBurst = completedPresence * Math.max(0, Math.min(1, (sample.scale - 1) / 0.18));
      const sparkActivity = Math.max(creatingPresence * (0.55 + (1 - Math.min(1, sample.radialScale)) * 0.45), completionBurst);
      sparks.visible = sparkActivity > 0.001;
      sparks.scale.setScalar(creatingPresence > completionBurst ? 0.8 + sample.radialScale * 0.5 : 1 + completionBurst * 0.6);
      sparks.rotation.y = flowTime * (creatingPresence > completionBurst ? 0.8 : 0.25);
      sparks.rotation.x = Math.sin(flowTime * 0.7) * 0.12;
      sparkMaterial.color.lerp(colors[currentState], Math.min(1, delta * 6));
      sparkMaterial.opacity = Math.min(1, sparkActivity * 0.78 * glow);

      ring.visible = sample.ringOpacity > 0.001;
      ring.scale.setScalar(0.72 + sample.ringProgress * 1.35);
      ring.rotation.z = sample.rotationY * 0.35;
      ringMaterial.color.lerp(colors[currentState], Math.min(1, delta * 5));
      ringMaterial.opacity = Math.min(1, sample.ringOpacity * 0.68 * glow);

      waitingMotes.visible = sample.waitingOpacity > 0.001;
      waitingMotes.scale.set(1, 1 + Math.sin(flowTime * 0.8) * 0.04, 1);
      waitingMotes.rotation.z = Math.sin(flowTime * 0.35) * 0.015;
      waitingMoteMaterial.color.lerp(colors.waiting, Math.min(1, delta * 5));
      waitingMoteMaterial.opacity = Math.min(1, sample.waitingOpacity * 0.52 * glow);
      waitingMoteMaterial.size = Math.max(0.032, sample.pointSize * 1.35);

      renderer.render(scene, camera);
      animation = requestAnimationFrame(render);
    };

    render();
    return () => {
      cancelAnimationFrame(animation);
      geometry.dispose();
      material.dispose();
      pulseGeometry.dispose();
      pulseMaterial.dispose();
      listeningSourceGeometry.dispose();
      listeningSourceMaterial.dispose();
      sparkGeometry.dispose();
      sparkMaterial.dispose();
      ringGeometry.dispose();
      ringMaterial.dispose();
      waitingMoteGeometry.dispose();
      waitingMoteMaterial.dispose();
      renderer.dispose();
    };
  }, [fallback, webglUnavailable]);

  if (fallback || webglUnavailable) {
    return (
      <div className={`entity-fallback ${state}${reducedMotion ? " motion-reduced" : ""}`} role="img" aria-label={`ALMA is ${state}`}>
        <pre aria-hidden="true">{FALLBACK_ART[state].join("\n")}</pre>
      </div>
    );
  }

  return <canvas ref={canvasRef} className="entity-canvas" role="img" aria-label={`ALMA is ${state}`} />;
}
