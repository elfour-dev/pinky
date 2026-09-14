import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { BookOpen, Box, ChevronRight, CirclePause, Database, FileText, FolderKey, KeyRound, LockKeyhole, MessageSquare, OctagonX, Play, Plus, Search, ShieldCheck, Square, X } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { AsciiEntity } from "./AsciiEntity";
import { deriveEntityState } from "./entity";
import { mergeTaskEvents } from "./events";
import type { CitationPassage, RuntimeStatus, SearchHit, SetupVaultResponse, SourceSummary, TaskEvent, VaultPaths } from "./types";

const EMPTY_STATUS: RuntimeStatus = { vault_mounted: false, setup_in_progress: false, vault_registered: false, vault_id: null, unlock_error: null, task_journal_error: null, watcher_error: null, prerequisites: { gocryptfs: false, podman: false, vulkan: false, secret_service: false } };
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
  const [sources, setSources] = useState<SourceSummary[]>([]);
  const [message, setMessage] = useState("");
  const [searchHits, setSearchHits] = useState<SearchHit[]>([]);
  const [searchError, setSearchError] = useState("");
  const [searching, setSearching] = useState(false);
  const [citation, setCitation] = useState<CitationPassage | null>(null);
  const [setupOpen, setSetupOpen] = useState(false);
  const [setupPaths, setSetupPaths] = useState<VaultPaths>({ cipher_dir: "", mount_dir: "" });
  const [passphrase, setPassphrase] = useState("");
  const [passphraseConfirmation, setPassphraseConfirmation] = useState("");
  const [setupError, setSetupError] = useState("");
  const [setupResult, setSetupResult] = useState<SetupVaultResponse | null>(null);
  const [setupRunning, setSetupRunning] = useState(false);
  const [sourceOpen, setSourceOpen] = useState(false);
  const [approvedRoot, setApprovedRoot] = useState("");
  const [sourcePath, setSourcePath] = useState("");
  const [sourceError, setSourceError] = useState("");
  const [listening, setListening] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(() => matchMedia("(prefers-reduced-motion: reduce)").matches);
  const tasks = useMemo(() => Object.values(events.reduce<Record<string, TaskEvent>>((latest, event) => ({ ...latest, [event.task_id]: event }), {})).sort((a, b) => b.sequence - a.sequence), [events]);
  const entityState = deriveEntityState(tasks, listening);

  useEffect(() => {
    if (IS_TAURI) void invoke<RuntimeStatus>("runtime_status").then((runtime) => {
      setStatus(runtime);
      if (runtime.vault_mounted) void invoke<SourceSummary[]>("list_sources").then(setSources).catch(() => undefined);
    }).catch(() => setStatus(EMPTY_STATUS));
    if (IS_TAURI) void invoke<TaskEvent[]>("task_snapshot").then((snapshot) => setEvents((current) => mergeTaskEvents(current, snapshot))).catch(() => undefined);
    const unlisten = IS_TAURI
      ? listen<TaskEvent>("pinky://task-event", ({ payload }) => {
        setEvents((current) => mergeTaskEvents(current, [payload]));
        if (payload.state === "completed" && payload.phase.name === "local ingestion") {
          void invoke<SourceSummary[]>("list_sources").then(setSources).catch(() => undefined);
        }
      }).catch(() => () => undefined)
      : Promise.resolve(() => undefined);
    const media = matchMedia("(prefers-reduced-motion: reduce)");
    const onMotion = () => setReducedMotion(media.matches);
    media.addEventListener("change", onMotion);
    return () => { void unlisten.then((stop) => stop()); media.removeEventListener("change", onMotion); };
  }, []);
  useEffect(() => {
    if (!IS_TAURI || !status.setup_in_progress) return;
    const timer = window.setInterval(() => void invoke<RuntimeStatus>("runtime_status").then(setStatus).catch(() => undefined), 250);
    return () => window.clearInterval(timer);
  }, [status.setup_in_progress]);
  useEffect(() => {
    if (!IS_TAURI || !status.vault_mounted) return;
    const timer = window.setInterval(() => void invoke<RuntimeStatus>("runtime_status").then(setStatus).catch(() => undefined), 2_000);
    return () => window.clearInterval(timer);
  }, [status.vault_mounted]);

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
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const query = message.trim();
    if (!query || !IS_TAURI || !status.vault_mounted) return;
    setSearchError(""); setSearching(true);
    try {
      setSearchHits(await invoke<SearchHit[]>("search_sources", { query, limit: 8 }));
    } catch (error) { setSearchError(String(error)); }
    finally { setSearching(false); }
  };
  const showCitation = async (uri: string) => {
    if (!IS_TAURI) return;
    setSearchError("");
    try { setCitation(await invoke<CitationPassage>("open_citation", { citationUri: uri })); }
    catch (error) { setSearchError(String(error)); }
  };
  const closeSetup = () => {
    if (setupRunning) return;
    setPassphrase(""); setPassphraseConfirmation(""); setSetupError(""); setSetupOpen(false);
  };
  const openSetup = async () => {
    setPassphrase(""); setPassphraseConfirmation(""); setSetupError(""); setSetupResult(null); setSetupOpen(true);
    if (IS_TAURI) {
      const paths = await invoke<VaultPaths>("default_vault_paths").catch(() => null);
      if (paths) setSetupPaths(paths);
    } else {
      setSetupPaths({ cipher_dir: "/home/you/.local/share/pinky/vault-cipher", mount_dir: "/home/you/.cache/pinky/vault-mounted" });
    }
  };
  const runSetup = async (event: FormEvent) => {
    event.preventDefault(); setSetupError("");
    if (passphrase.length < 12) { setSetupError("Use a recovery passphrase containing at least 12 characters."); return; }
    if (passphrase !== passphraseConfirmation) { setSetupError("The recovery passphrases do not match."); return; }
    if (!IS_TAURI) { setSetupError("Vault creation runs only in the native Pinky application, not this browser preview."); return; }
    setSetupRunning(true);
    try {
      const result = await invoke<SetupVaultResponse>("setup_vault", { request: { ...setupPaths, recovery_passphrase: passphrase } });
      setPassphrase(""); setPassphraseConfirmation(""); setSetupResult(result);
      setStatus(await invoke<RuntimeStatus>("runtime_status"));
      setSources(await invoke<SourceSummary[]>("list_sources"));
    } catch (error) {
      setSetupError(String(error));
    } finally { setSetupRunning(false); }
  };
  const retryUnlock = async () => {
    if (!IS_TAURI) return;
    setStatus((current) => ({ ...current, setup_in_progress: true, unlock_error: null }));
    try {
      await invoke("unlock_vault");
    } catch (error) {
      const refreshed = await invoke<RuntimeStatus>("runtime_status").catch(() => null);
      setStatus((current) => ({ ...(refreshed || current), setup_in_progress: false, unlock_error: String(error) }));
      return;
    }
    const refreshed = await invoke<RuntimeStatus>("runtime_status").catch(() => null);
    if (refreshed) {
      setStatus(refreshed);
      if (refreshed.vault_mounted) setSources(await invoke<SourceSummary[]>("list_sources").catch(() => []));
    }
  };
  const openSource = () => {
    setSourceError(""); setApprovedRoot(""); setSourcePath(""); setSourceOpen(true);
  };
  const closeSource = () => { setSourceError(""); setSourceOpen(false); };
  const runIngestion = async (event: FormEvent) => {
    event.preventDefault(); setSourceError("");
    if (!IS_TAURI) { setSourceError("Local ingestion is available only in the native application."); return; }
    try {
      await invoke<string>("ingest_local_file", { request: { approved_root: approvedRoot, source_path: sourcePath } });
      closeSource();
    } catch (error) { setSourceError(String(error)); }
  };

  return <main className="app-shell">
    <a className="skip-link" href="#conversation">Skip to conversation</a>
    <aside className="left-panel" aria-label="Knowledge navigation">
      <header className="brand"><span className="brand-mark">P</span><div><strong>PINKY</strong><small>PRIVATE INTELLIGENCE</small></div></header>
      <button className="new-chat" disabled title="Chat arrives after retrieval and local model setup"><Plus size={15} /> New conversation</button>
      <nav>
        <NavGroup icon={<MessageSquare />} label="Chats" count="0"><p className="empty-nav">No conversations yet</p></NavGroup>
        <NavGroup icon={<Database />} label="Sources" count={String(sources.length)}><button className="nav-row" disabled={!status.vault_mounted} onClick={openSource}><Plus size={13} /> Add source</button>{sources.map((source) => <div className="source-row" key={source.source_id} title={source.canonical_uri}><FileText size={12} /><div><strong>{source.display_name}</strong><small>{source.state === "active" ? `${source.chunk_count} chunk${source.chunk_count === 1 ? "" : "s"}` : source.state}</small></div></div>)}</NavGroup>
        <NavGroup icon={<BookOpen />} label="Dossiers" count="0" />
        <NavGroup icon={<FolderKey />} label="Workspaces" count="0"><button className="nav-row"><Plus size={13} /> Approve directory</button></NavGroup>
      </nav>
      <div className={`vault-card ${status.vault_mounted ? "ready" : "locked"}`}><ShieldCheck size={17} /><div><strong>{status.vault_mounted ? "Vault unlocked" : status.vault_registered ? "Vault registered" : "Vault locked"}</strong><small>{status.vault_mounted ? "Encrypted storage available" : status.vault_registered ? "Waiting for encrypted storage to unlock" : "Setup required before data can be retained"}</small></div></div>
    </aside>

    <section className="centre-panel" id="conversation" aria-label="Conversation">
      <div className="topbar"><div className="crumb">Retained knowledge <ChevronRight size={13} /> <span>Lexical search</span></div><button className="icon-button" aria-label="Search retained sources" title="Search retained sources"><Search size={16} /></button></div>
      <div className="conversation" role="region" aria-label="Conversation and search results" tabIndex={0}>
        <div className="entity-stage"><AsciiEntity state={entityState} reducedMotion={reducedMotion} /><span className={`state-pill ${entityState}`} aria-live="polite"><i /> {entityState}</span></div>
        <div className="welcome"><p className="eyebrow">ENCRYPTED · LOCAL · SOURCE-GROUNDED</p><h1>What should we understand<br />or create?</h1><p>Pinky retains approved evidence inside your encrypted vault and shows every operation while it works.</p></div>
        {!status.vault_mounted && (status.vault_registered ? <div className="blocking-question" role="alert"><KeyRound size={19} /><div><strong>{status.setup_in_progress ? "Unlocking your encrypted vault" : "Your registered vault is locked"}</strong><p>{status.unlock_error || "Pinky is retrieving its protected key from Linux Secret Service."}</p></div><button disabled={status.setup_in_progress || !status.prerequisites.gocryptfs || !status.prerequisites.secret_service} onClick={retryUnlock}>{status.setup_in_progress ? "Unlocking…" : "Retry unlock"}</button></div> : <div className="blocking-question" role="alert"><FolderKey size={19} /><div><strong>Set up the encrypted vault to begin</strong><p>Ingestion, conversations, generation, and logs stay disabled until gocryptfs and Secret Service are ready.</p></div><button onClick={openSetup}>Start setup</button></div>)}
        {status.vault_mounted && !searchHits.length && <div className="capability-notice"><Database size={17} /><div><strong>Encrypted source search is ready</strong><p>Add local text sources, then search their retained passages below. Cited chat will follow when the local model runtime is connected.</p></div><button onClick={openSource}>Add source</button></div>}
        {searchError && <p className="search-error" role="alert">{searchError}</p>}
        {!!searchHits.length && <section className="search-results" aria-label="Retained source search results"><header><p className="eyebrow">MATCHING EVIDENCE</p><span>{searchHits.length} passage{searchHits.length === 1 ? "" : "s"}</span></header>{searchHits.map((hit) => <article key={hit.chunk_id}><div><FileText size={14} /><strong>{hit.display_name}</strong>{hit.heading && <span>{hit.heading}</span>}</div><p>{hit.passage}</p><button onClick={() => void showCitation(hit.citation_uri)}>{formatCoordinates(hit.coordinates)} · Open retained citation</button></article>)}</section>}
      </div>
      <form className="composer" onSubmit={submit}>
        <textarea aria-label="Search retained sources" value={message} onChange={(event) => setMessage(event.target.value)} onFocus={() => setListening(true)} onBlur={() => setListening(false)} placeholder={status.vault_mounted ? "Search your retained sources…" : "Unlock the encrypted vault to start…"} disabled={!status.vault_mounted || searching} onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); event.currentTarget.form?.requestSubmit(); } }} />
        <div className="composer-footer"><div><button type="button" className="tool-chip" disabled={!status.vault_mounted} onClick={openSource}><Plus size={14} /> Attach source</button><span>{status.vault_mounted ? "Lexical retrieval · exact citations" : "Encrypted vault required"}</span></div><button className="send" aria-label="Search" disabled={!status.vault_mounted || searching || !message.trim()}>{searching ? "…" : "↑"}</button></div>
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
          {task.error && <p className="task-error">{task.error.message}</p>}
        </article>)}
      </div>
      <section className="runtime"><p className="eyebrow">RUNTIME</p>{Object.entries(status.prerequisites).map(([name, available]) => <div key={name}><span>{name.replace("_", " ")}</span><b className={available ? "ok" : "missing"}>{available ? "ready" : "missing"}</b></div>)}<div><span>task journal</span><b className={status.task_journal_error ? "missing" : "ok"} title={status.task_journal_error || undefined}>{status.task_journal_error ? "error" : status.vault_mounted ? "durable" : "locked"}</b></div><div><span>file watcher</span><b className={status.watcher_error ? "missing" : "ok"} title={status.watcher_error || undefined}>{status.watcher_error ? "error" : status.vault_mounted ? "watching" : "locked"}</b></div></section>
      <section className="log-panel"><div className="log-title"><span>Live event log</span><span>schema 1.0</span></div><ActivityTerminal events={events} /></section>
    </aside>
    {setupOpen && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) closeSetup(); }}>
      <section className="setup-modal" role="dialog" aria-modal="true" aria-labelledby="setup-title">
        <header><div><p className="eyebrow">ENCRYPTED VAULT</p><h2 id="setup-title">Create your private vault</h2></div><button aria-label="Close setup" disabled={setupRunning} onClick={closeSetup}><X size={17} /></button></header>
        {setupResult ? <div className="setup-complete"><ShieldCheck size={34} /><h3>Vault created and mounted</h3><p>Your recovery envelope is stored at:</p><code>{setupResult.recovery_path}</code><p>Keep your recovery passphrase somewhere safe. Pinky does not retain it.</p><button onClick={closeSetup}>Continue</button></div> : <form onSubmit={runSetup}>
          <div className="setup-intro"><LockKeyhole size={21} /><p>Pinky generates a random 256-bit key. Secret Service protects it for daily use; your passphrase protects a separate recovery envelope.</p></div>
          <label>Encrypted data directory<input value={setupPaths.cipher_dir} onChange={(event) => setSetupPaths((paths) => ({ ...paths, cipher_dir: event.target.value }))} spellCheck={false} required /></label>
          <label>Unlocked mount directory<input value={setupPaths.mount_dir} onChange={(event) => setSetupPaths((paths) => ({ ...paths, mount_dir: event.target.value }))} spellCheck={false} required /></label>
          <div className="passphrase-grid"><label>Recovery passphrase<input type="password" autoComplete="new-password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} minLength={12} required /></label><label>Confirm passphrase<input type="password" autoComplete="new-password" value={passphraseConfirmation} onChange={(event) => setPassphraseConfirmation(event.target.value)} minLength={12} required /></label></div>
          <div className="setup-warning"><KeyRound size={17} /><span>If both Secret Service and this passphrase are lost, the vault cannot be recovered.</span></div>
          {(!status.prerequisites.gocryptfs || !status.prerequisites.secret_service) && <p className="setup-error" role="alert">Install gocryptfs and Secret Service tools before creating the vault.</p>}
          {setupError && <p className="setup-error" role="alert">{setupError}</p>}
          <footer><button type="button" disabled={setupRunning} onClick={closeSetup}>Cancel</button><button className="primary" disabled={setupRunning || !status.prerequisites.gocryptfs || !status.prerequisites.secret_service}>{setupRunning ? "Creating encrypted vault…" : "Create vault"}</button></footer>
        </form>}
      </section>
    </div>}
    {sourceOpen && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) closeSource(); }}>
      <section className="setup-modal source-modal" role="dialog" aria-modal="true" aria-labelledby="source-title">
        <header><div><p className="eyebrow">APPROVED LOCAL SOURCE</p><h2 id="source-title">Retain a local text file</h2></div><button aria-label="Close source setup" onClick={closeSource}><X size={17} /></button></header>
        <form onSubmit={runIngestion}>
          <div className="setup-intro"><FolderKey size={21} /><p>The approved root is the directory Pinky may access. The source must resolve inside it; devices, sockets, directories, and symlink escapes are rejected.</p></div>
          <label>Approved directory<input value={approvedRoot} onChange={(event) => setApprovedRoot(event.target.value)} placeholder="/home/you/Documents" spellCheck={false} required /></label>
          <label>Source file<input value={sourcePath} onChange={(event) => setSourcePath(event.target.value)} placeholder="/home/you/Documents/notes.md" spellCheck={false} required /></label>
          <p className="source-support">Currently extractable: UTF-8 text, Markdown, logs, source code, JSON, YAML, XML, HTML, and CSV. Other formats are safely archived and marked unsupported.</p>
          {sourceError && <p className="setup-error" role="alert">{sourceError}</p>}
          <footer><button type="button" onClick={closeSource}>Cancel</button><button className="primary">Archive and extract</button></footer>
        </form>
      </section>
    </div>}
    {citation && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) setCitation(null); }}>
      <section className="setup-modal citation-modal" role="dialog" aria-modal="true" aria-labelledby="citation-title">
        <header><div><p className="eyebrow">RETAINED SOURCE SNAPSHOT</p><h2 id="citation-title">{citation.display_name}</h2></div><button aria-label="Close citation" onClick={() => setCitation(null)}><X size={17} /></button></header>
        <div className="citation-provenance"><span>{citation.mime_type}</span><span>{formatCoordinates(citation.coordinates)}</span><span>Retrieved {new Date(citation.retrieved_at).toLocaleString()}</span></div>
        <pre>{citation.passage}</pre><code>{citation.citation_uri}</code><p className="citation-origin" title={citation.canonical_uri}>{citation.canonical_uri}</p>
      </section>
    </div>}
  </main>;
}

function formatCoordinates(coordinates: SearchHit["coordinates"]) {
  if (!coordinates?.line_start) return "Chunk passage";
  return coordinates.line_start === coordinates.line_end ? `Line ${coordinates.line_start}` : `Lines ${coordinates.line_start}–${coordinates.line_end}`;
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
