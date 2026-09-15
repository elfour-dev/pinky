import { useEffect, useRef } from "react";
import * as THREE from "three";
import type { EntityState } from "./entity";
export function AsciiEntity({ state, reducedMotion }: { state: EntityState; reducedMotion: boolean }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const fallback = typeof WebGLRenderingContext === "undefined";
  useEffect(() => {
    if (fallback || !canvasRef.current) return;
    const canvas = canvasRef.current;
    const renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true });
    renderer.setPixelRatio(Math.min(devicePixelRatio, 1.5));
    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(48, 1, 0.1, 100); camera.position.z = 5;
    const geometry = new THREE.BufferGeometry(); const positions: number[] = []; const pointCount = 1100;
    for (let index = 0; index < pointCount; index++) {
      const phi = Math.acos(1 - 2 * (index + 0.5) / pointCount); const theta = Math.PI * (1 + Math.sqrt(5)) * index; const radius = 1.42 + 0.12 * Math.sin(theta * 3);
      positions.push(radius * Math.cos(theta) * Math.sin(phi), radius * Math.sin(theta) * Math.sin(phi), radius * Math.cos(phi));
    }
    geometry.setAttribute("position", new THREE.Float32BufferAttribute(positions, 3));
    const palette: Record<EntityState, number> = { idle: 0xe9a3bf, listening: 0xffc2d6, thinking: 0xc89cff, researching: 0x7de2ff, creating: 0xffc96b, waiting: 0xf4d97b, cancelling: 0x887681, error: 0xff5268, completed: 0x8af0bb };
    const material = new THREE.PointsMaterial({ color: palette[state], size: 0.025, transparent: true, opacity: 0.9 });
    const cloud = new THREE.Points(geometry, material); scene.add(cloud); let frame = 0; let animation = 0;
    const render = () => {
      const rect = canvas.getBoundingClientRect();
      if (canvas.width !== rect.width || canvas.height !== rect.height) { renderer.setSize(rect.width, rect.height, false); camera.aspect = rect.width / Math.max(rect.height, 1); camera.updateProjectionMatrix(); }
      frame += reducedMotion ? 0 : 0.01; const speed = state === "thinking" ? 2.6 : state === "researching" ? 1.4 : 0.35;
      cloud.rotation.y = frame * speed; cloud.rotation.x = Math.sin(frame * 0.35) * 0.15;
      const pulse = state === "cancelling" ? Math.max(0.7, 1 - frame * 0.005) : 1 + Math.sin(frame * 1.2) * (state === "idle" ? 0.025 : 0.055);
      cloud.scale.setScalar(reducedMotion ? 1 : pulse); renderer.render(scene, camera); animation = requestAnimationFrame(render);
    };
    render(); return () => { cancelAnimationFrame(animation); geometry.dispose(); material.dispose(); renderer.dispose(); };
  }, [fallback, reducedMotion, state]);
  if (fallback) return <div className="entity-fallback" role="img" aria-label={`ALMA is ${state}`}><pre>{["  .-~~~~-.", " /  *  *  \\", "|    ~     |", " \\  ___  /", "  '-.__.-'"].join("\n")}</pre></div>;
  return <canvas ref={canvasRef} className="entity-canvas" role="img" aria-label={`ALMA is ${state}`} />;
}
