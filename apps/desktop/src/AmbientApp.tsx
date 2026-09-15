import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AsciiEntity } from "./AsciiEntity";
import { AMBIENT_STATES, ambientPalette, cloneAmbientSettings, DEFAULT_AMBIENT_SETTINGS, loadAmbientSettings, saveAmbientSettings, type AmbientExpression, type AmbientSettings } from "./ambientSettings";
import type { EntityState } from "./entity";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

function hexToRgba(hex: string, opacity: number): string {
  const red = Number.parseInt(hex.slice(1, 3), 16);
  const green = Number.parseInt(hex.slice(3, 5), 16);
  const blue = Number.parseInt(hex.slice(5, 7), 16);
  return `rgba(${red}, ${green}, ${blue}, ${opacity})`;
}

function useAmbientState(expression: AmbientExpression, cycleSeconds: number): EntityState {
  const [cycleIndex, setCycleIndex] = useState(0);

  useEffect(() => {
    setCycleIndex(0);
    if (expression !== "cycle") return;
    const timer = window.setInterval(() => setCycleIndex((current) => (current + 1) % AMBIENT_STATES.length), cycleSeconds * 1_000);
    return () => window.clearInterval(timer);
  }, [cycleSeconds, expression]);

  return expression === "cycle" ? AMBIENT_STATES[cycleIndex] : expression;
}

function Slider({ label, value, minimum, maximum, step, suffix, onChange }: { label: string; value: number; minimum: number; maximum: number; step: number; suffix: string; onChange: (value: number) => void }) {
  return <label className="ambient-slider"><span>{label}<output>{value.toFixed(1)}{suffix}</output></span><input type="range" value={value} min={minimum} max={maximum} step={step} onChange={(event) => onChange(Number(event.target.value))} /></label>;
}

function ColorField({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  return <label className="ambient-color"><span>{label}</span><input type="color" value={value} onChange={(event) => onChange(event.target.value)} /><code>{value}</code></label>;
}

export function AmbientApp() {
  const [settings, setSettings] = useState<AmbientSettings>(() => loadAmbientSettings());
  const [clickThrough, setClickThrough] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [systemReducedMotion, setSystemReducedMotion] = useState(() => matchMedia("(prefers-reduced-motion: reduce)").matches);
  const state = useAmbientState(settings.expression, settings.cycleSeconds);
  const palette = useMemo(() => ambientPalette(settings), [settings]);

  useEffect(() => {
    document.documentElement.classList.add("ambient-window");
    return () => document.documentElement.classList.remove("ambient-window");
  }, []);

  useEffect(() => saveAmbientSettings(settings), [settings]);

  useEffect(() => {
    const media = matchMedia("(prefers-reduced-motion: reduce)");
    const onMotion = () => setSystemReducedMotion(media.matches);
    media.addEventListener("change", onMotion);
    return () => media.removeEventListener("change", onMotion);
  }, []);

  useEffect(() => {
    if (!IS_TAURI) return;
    void getCurrentWindow().setIgnoreCursorEvents(clickThrough).catch(() => undefined);
  }, [clickThrough]);

  useEffect(() => {
    if (!IS_TAURI) return;
    let stop: (() => void) | undefined;
    void listen("pinky://ambient-configure", () => {
      setClickThrough(false);
      setSettingsOpen(true);
    }).then((unlisten) => { stop = unlisten; }).catch(() => undefined);
    return () => stop?.();
  }, []);

  const updateSettings = (update: (current: AmbientSettings) => AmbientSettings) => setSettings((current) => update(current));
  const updateColor = (key: keyof AmbientSettings["colors"], value: string) => updateSettings((current) => ({ ...current, colors: { ...current.colors, [key]: value } }));
  const updateMotion = (key: keyof AmbientSettings["motion"], value: number) => updateSettings((current) => ({ ...current, motion: { ...current.motion, [key]: value } }));
  const pinToDesktop = () => { setSettingsOpen(false); setClickThrough(true); };
  const reset = () => setSettings(cloneAmbientSettings(DEFAULT_AMBIENT_SETTINGS));
  const shellStyle = {
    "--ambient-background": settings.backgroundOpacity > 0 ? hexToRgba(settings.backgroundColor, settings.backgroundOpacity) : "transparent",
    "--ambient-core": settings.colors.core,
  } as CSSProperties;

  return <main className="ambient-shell" style={shellStyle} onDoubleClick={() => { if (!clickThrough) setSettingsOpen(true); }}>
    <div className="ambient-entity-stage" style={{ transform: `scale(${settings.scale})` }} aria-label={`ALMA is ${state}`}>
      <AsciiEntity state={state} reducedMotion={systemReducedMotion} palette={palette} motion={settings.motion} />
    </div>
    {!clickThrough && settingsOpen && <section className="ambient-controls" aria-label="Ambient ALMA settings">
      <header className="ambient-controls-header">
        <strong data-tauri-drag-region>AMBIENT ALMA</strong>
        <button type="button" onClick={() => setSettingsOpen(false)} aria-label="Close ambient ALMA settings">×</button>
      </header>
      <p className="ambient-help">Tune her colours and movement, then pin her behind your windows. Reopen Ambient from Pinky to edit her again.</p>
      <label className="ambient-select">Expression<select value={settings.expression} onChange={(event) => updateSettings((current) => ({ ...current, expression: event.target.value as AmbientExpression }))}>{AMBIENT_STATES.map((expression) => <option value={expression} key={expression}>{expression}</option>)}<option value="cycle">Gentle cycle</option></select></label>
      <div className="ambient-control-grid">
        <Slider label="Flow" value={settings.motion.flow} minimum={0.1} maximum={2} step={0.1} suffix="×" onChange={(value) => updateMotion("flow", value)} />
        <Slider label="Reactions" value={settings.motion.reactions} minimum={0} maximum={2} step={0.1} suffix="×" onChange={(value) => updateMotion("reactions", value)} />
        <Slider label="Glow" value={settings.motion.glow} minimum={0.2} maximum={2} step={0.1} suffix="×" onChange={(value) => updateMotion("glow", value)} />
        <Slider label="Scale" value={settings.scale} minimum={0.45} maximum={1.7} step={0.05} suffix="×" onChange={(value) => updateSettings((current) => ({ ...current, scale: value }))} />
      </div>
      <div className="ambient-colors">
        <ColorField label="Core" value={settings.colors.core} onChange={(value) => updateColor("core", value)} />
        <ColorField label="Active" value={settings.colors.active} onChange={(value) => updateColor("active", value)} />
        <ColorField label="Research" value={settings.colors.research} onChange={(value) => updateColor("research", value)} />
        <ColorField label="Highlight" value={settings.colors.highlight} onChange={(value) => updateColor("highlight", value)} />
        <ColorField label="Alert" value={settings.colors.alert} onChange={(value) => updateColor("alert", value)} />
      </div>
      <div className="ambient-control-grid">
        <Slider label="Backdrop" value={settings.backgroundOpacity} minimum={0} maximum={0.85} step={0.05} suffix="" onChange={(value) => updateSettings((current) => ({ ...current, backgroundOpacity: value }))} />
        <label className="ambient-color"><span>Backdrop colour</span><input type="color" value={settings.backgroundColor} onChange={(event) => updateSettings((current) => ({ ...current, backgroundColor: event.target.value }))} /><code>{settings.backgroundColor}</code></label>
      </div>
      <label className="ambient-check"><input type="checkbox" checked={systemReducedMotion} onChange={(event) => setSystemReducedMotion(event.target.checked)} /> Reduce motion</label>
      <footer className="ambient-controls-footer"><button type="button" onClick={reset}>Reset</button><button type="button" className="ambient-pin" onClick={pinToDesktop}>Pin to desktop</button></footer>
    </section>}
    {!clickThrough && !settingsOpen && <button type="button" className="ambient-show-controls" onClick={() => setSettingsOpen(true)}>Configure</button>}
  </main>;
}
