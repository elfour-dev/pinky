import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { BookOpen, Box, ChevronRight, CirclePause, Database, FolderKey, MessageSquare, OctagonX, Play, Plus, Search, ShieldCheck, Square } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { AsciiEntity } from "./AsciiEntity";
import { deriveEntityState } from "./entity";
import type { RuntimeStatus, TaskEvent } from "./types";

const EMPTY_STATUS: RuntimeStatus = { vault_mounted: false, prerequisites: { gocryptfs: false, podman: false, vulkan: false, secret_service: false } };
const IS_TAURI = "__TAURI_INTERNALS__" in window;

function ActivityTerminal({ events }: { events: TaskEvent[] }) {
  const target = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal>();
  const written = useRef(0);
  useEffect(() => {
    if (!target.current) return;
    terminal.current = new Terminal({ convertEol: true, disableStdin: true, fontFamily: "'IBM Plex Mono', monospace", fontSize: 11, rows: 7, theme: { background: "#0b0c10", foreground: "#9ba3ae", cyan: "#e29bb9" } });
    terminal.current.open(target.current);
    terminal.current.writeln("PINKY TASK EVENT STREAM");
    return () => { terminal.current?.dispose(); terminal.current = undefined; written.current = 0; };
  }, []);
  useEffect(() => {
    for (const event of events.slice(written.current)) terminal.current?.writeln(`${String(event.sequence).padStart(4, "0")}  ${event.state.padEnd(12)}  ${event.phase.activity}`);
    written.current = events.length;
  }, [events]);
  return <div className="terminal" ref={target} aria-label="Task event log" />;
}

export function App() {
  const [status, setStatus] = useState(EMPTY_STATUS);
  const [events, setEvents] = useState<TaskEvent[]>([]);
  const [message, setMessage] = useState("");
  const [listening, setListening] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(() => matchMedia("(prefers-reduced-motion: reduce)").matches);
  const tasks = useMemo(() => Object.values(events.reduce<Record<string, TaskEvent>>((latest, event) => ({ ...latest, [event.task_id]: event }), {})).sort((a, b) => b.sequence - a.sequence), [events]);
  const entityState = deriveEntityState(tasks, listening);

  useEffect(() => {
    if (IS_TAURI) void invoke<RuntimeStatus>("runtime_status").then(setStatus).catch(() => setStatus(EMPTY_STATUS));
    const unlisten = IS_TAURI
      ? listen<TaskEvent>("pinky://task-event", ({ payload }) => setEvents((current) => [...current, payload].slice(-500))).catch(() => () => undefined)
      : Promise.resolve(() => undefined);
    const media = matchMedia("(prefers-reduced-motion: reduce)");
    const onMotion = () => setReducedMotion(media.matches);
    media.addEventListener("change", onMotion);
    return () => { void unlisten.then((stop) => stop()); media.removeEventListener("change", onMotion); };
  }, []);

  const startCheck = async () => {
    if (!IS_TAURI) { browserDemo(setEvents); return; }
    await invoke<string>("start_system_check").catch(() => undefined);
  };
  const stop = (taskId: string) => {
    if (IS_TAURI) { void invoke("cancel_task", { taskId }).catch(() => undefined); return; }
    clearInterval(demoTimers.get(taskId)); demoTimers.delete(taskId);
    setEvents((current) => [...current, cancelledPreview(current, taskId)]);
  };
  const pause = (taskId: string, paused: boolean) => { if (IS_TAURI) void invoke("pause_task", { taskId, paused }).catch(() => undefined); };
  const stopAll = () => {
    if (!window.confirm("Stop every cancellable task? Partial artifacts will be quarantined.")) return;
    if (IS_TAURI) { void invoke("cancel_all_tasks").catch(() => undefined); return; }
    for (const timer of demoTimers.values()) clearInterval(timer);
    const ids = [...demoTimers.keys()]; demoTimers.clear();
    setEvents((current) => [...current, ...ids.map((id) => cancelledPreview(current, id))]);
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!message.trim()) return;
    if (!status.vault_mounted) { setMessage(""); return; }
  };

  return <main className="app-shell">
    <a className="skip-link" href="#conversation">Skip to conversation</a>
    <aside className="left-panel" aria-label="Knowledge navigation">
      <header className="brand"><span className="brand-mark">P</span><div><strong>PINKY</strong><small>PRIVATE INTELLIGENCE</small></div></header>
      <button className="new-chat"><Plus size={15} /> New conversation</button>
      <nav>
        <NavGroup icon={<MessageSquare />} label="Chats" count="0"><p className="empty-nav">No conversations yet</p></NavGroup>
        <NavGroup icon={<Database />} label="Sources" count="0"><button className="nav-row"><Plus size={13} /> Add source</button></NavGroup>
        <NavGroup icon={<BookOpen />} label="Dossiers" count="0" />
        <NavGroup icon={<FolderKey />} label="Workspaces" count="0"><button className="nav-row"><Plus size={13} /> Approve directory</button></NavGroup>
      </nav>
      <div className={`vault-card ${status.vault_mounted ? "ready" : "locked"}`}><ShieldCheck size={17} /><div><strong>{status.vault_mounted ? "Vault unlocked" : "Vault locked"}</strong><small>{status.vault_mounted ? "Encrypted storage available" : "Setup required before data can be retained"}</small></div></div>
    </aside>

    <section className="centre-panel" id="conversation" aria-label="Conversation">
      <div className="topbar"><div className="crumb">New conversation <ChevronRight size={13} /> <span>Local only</span></div><button className="icon-button" aria-label="Search"><Search size={16} /></button></div>
      <div className="conversation">
        <div className="entity-stage"><AsciiEntity state={entityState} reducedMotion={reducedMotion} /><span className={`state-pill ${entityState}`} aria-live="polite"><i /> {entityState}</span></div>
        <div className="welcome"><p className="eyebrow">ENCRYPTED · LOCAL · SOURCE-GROUNDED</p><h1>What should we understand<br />or create?</h1><p>Pinky retains approved evidence inside your encrypted vault and shows every operation while it works.</p></div>
        {!status.vault_mounted && <div className="blocking-question" role="alert"><FolderKey size={19} /><div><strong>Set up the encrypted vault to begin</strong><p>Ingestion, conversations, generation, and logs stay disabled until gocryptfs and Secret Service are ready.</p></div><button>Start setup</button></div>}
      </div>
      <form className="composer" onSubmit={submit}>
        <textarea aria-label="Message Pinky" value={message} onChange={(event) => setMessage(event.target.value)} onFocus={() => setListening(true)} onBlur={() => setListening(false)} placeholder={status.vault_mounted ? "Ask from your sources or describe what to create…" : "Unlock the encrypted vault to start…"} disabled={!status.vault_mounted} />
        <div className="composer-footer"><div><button type="button" className="tool-chip"><Plus size={14} /> Attach</button><span>Sources cited automatically</span></div><button className="send" aria-label="Send message" disabled={!status.vault_mounted || !message.trim()}>↑</button></div>
      </form>
    </section>

    <aside className="right-panel" aria-label="Task activity">
      <div className="task-header"><div><p className="eyebrow">OPERATIONS</p><h2>Active tasks</h2></div><button className="stop-all" disabled={!tasks.some((task) => task.cancellable)} onClick={stopAll}><OctagonX size={14} /> Stop all</button></div>
      <div className="task-list">
        {!tasks.length && <div className="empty-tasks"><Box size={30} /><strong>No active operations</strong><p>Ingestion, research, generation, and tool work will appear here.</p><button onClick={startCheck}><Play size={13} /> Run system check</button></div>}
        {tasks.map((task) => <article className={`task-card ${task.state}`} key={task.task_id}>
          <div className="task-title"><i /><div><strong>{task.phase.name}</strong><small>{task.phase.activity}</small></div><span>{task.phase.progress == null ? "—" : `${Math.round(task.phase.progress * 100)}%`}</span></div>
          {task.phase.progress != null && <div className="progress"><span style={{ width: `${task.phase.progress * 100}%` }} /></div>}
          <div className="task-meta"><span>{task.permission_state}</span><span>{task.budget_state.replaceAll("_", " ")}</span></div>
          {task.cancellable && <div className="task-actions"><button onClick={() => pause(task.task_id, task.phase.activity !== "Paused")}><CirclePause size={13} /> {task.phase.activity === "Paused" ? "Resume" : "Pause"}</button><button onClick={() => stop(task.task_id)}><Square size={12} /> Stop</button></div>}
        </article>)}
      </div>
      <section className="runtime"><p className="eyebrow">RUNTIME</p>{Object.entries(status.prerequisites).map(([name, available]) => <div key={name}><span>{name.replace("_", " ")}</span><b className={available ? "ok" : "missing"}>{available ? "ready" : "missing"}</b></div>)}</section>
      <section className="log-panel"><div className="log-title"><span>Live event log</span><span>schema 1.0</span></div><ActivityTerminal events={events} /></section>
    </aside>
  </main>;
}

function NavGroup({ icon, label, count, children }: { icon: React.ReactNode; label: string; count: string; children?: React.ReactNode }) {
  return <section className="nav-group"><div className="nav-heading">{icon}<span>{label}</span><small>{count}</small></div>{children}</section>;
}

function browserDemo(setEvents: React.Dispatch<React.SetStateAction<TaskEvent[]>>) {
  const id = crypto.randomUUID(); let progress = 0; let sequence = 1;
  const push = (state: TaskEvent["state"], activity: string, cancellable: boolean) => setEvents((current) => [...current, { schema_major: 1, schema_minor: 0, sequence: sequence++, task_id: id, parent_id: null, timestamp: new Date().toISOString(), state, phase: { name: "System check", progress, activity }, tool_name: "prerequisite_probe", resource_uri: null, permission_state: "approved", budget_state: "within_budget", cancellable, error: null }]);
  push("running", "Checking local prerequisites", true);
  const timer = setInterval(() => { progress += 0.2; if (progress >= 1) { progress = 1; push("completed", "System check complete", false); clearInterval(timer); demoTimers.delete(id); } else push("running", "Inspecting runtime capabilities", true); }, 350);
  demoTimers.set(id, timer);
}

const demoTimers = new Map<string, ReturnType<typeof setInterval>>();

function cancelledPreview(events: TaskEvent[], taskId: string): TaskEvent {
  const latest = [...events].reverse().find((event) => event.task_id === taskId)!;
  return { ...latest, sequence: Math.max(0, ...events.map((event) => event.sequence)) + 1, timestamp: new Date().toISOString(), state: "cancelled", cancellable: false, phase: { ...latest.phase, progress: null, activity: "Cancelled in preview" } };
}
