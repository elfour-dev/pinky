import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowLeft, BookOpen, Box, ChevronRight, CirclePause, Cpu, Database, FileText, FolderKey, Grid2X2, KeyRound, List, LockKeyhole, MessageSquare, OctagonX, Pencil, Play, Plus, RefreshCw, Search, Settings2, ShieldCheck, Sparkles, Square, Trash2, X } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { AsciiEntity } from "./AsciiEntity";
import { deriveEntityState } from "./entity";
import { mergeTaskEvents } from "./events";
import { isReconnectableError } from "./reliability";
import type { AnswerEnvelope, AskQuestionResponse, AssetListResponse, AssetSort, AttachLlamaResponse, CitationPassage, ConversationDetail, ConversationMessage, ConversationSummary, HybridConfiguration, RetainedImage, RuntimeStatus, SearchHit, SetupVaultResponse, SourceSummary, TaskEvent, VaultPaths } from "./types";
import type { EntityState } from "./entity";
import { canAsk } from "./types";

const EMPTY_STATUS: RuntimeStatus = { vault_mounted: false, setup_in_progress: false, vault_registered: false, vault_id: null, unlock_error: null, task_journal_error: null, watcher_error: null, model_attach_in_progress: false, model_connected: false, model_provider: null, model_name: null, model_context_size: null, model_error: null, hybrid_configured: false, hybrid_model: null, prerequisites: { gocryptfs: false, podman: false, vulkan: false, secret_service: false } };
const IS_TAURI = "__TAURI_INTERNALS__" in window;
const ENTITY_STATES: EntityState[] = ["idle", "listening", "thinking", "researching", "creating", "waiting", "cancelling", "error", "completed"];

function taskAge(timestamp: string, now: number): string {
  const seconds = Math.max(0, Math.floor((now - Date.parse(timestamp)) / 1000));
  return seconds === 0 ? "updated now" : `updated ${seconds}s ago`;
}

function isFinishedTask(task: TaskEvent): boolean {
  return ["completed", "failed", "failed_interrupted", "cancelled"].includes(task.state);
}

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
  return <div className="terminal" ref={target} role="log" aria-live="polite" aria-label="Task event log" />;
}

export function App() {
  const [status, setStatus] = useState(EMPTY_STATUS);
  const [events, setEvents] = useState<TaskEvent[]>([]);
  const [sources, setSources] = useState<SourceSummary[]>([]);
  const [activeView, setActiveView] = useState<"conversation" | "assets">("conversation");
  const [assetCount, setAssetCount] = useState(0);
  const [assetRefreshKey, setAssetRefreshKey] = useState(0);
  const [message, setMessage] = useState("");
  const [submittedPrompt, setSubmittedPrompt] = useState<{ text: string; submittedAt: string } | null>(null);
  const [searchHits, setSearchHits] = useState<SearchHit[]>([]);
  const [searchError, setSearchError] = useState("");
  const [searching, setSearching] = useState(false);
  const [mode, setMode] = useState<"search" | "ask">("search");
  const [answer, setAnswer] = useState<AnswerEnvelope | null>(null);
  const [askError, setAskError] = useState("");
  const [asking, setAsking] = useState(false);
  const askRun = useRef(0);
  const conversationRef = useRef<HTMLDivElement>(null);
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);
  const [conversationMessages, setConversationMessages] = useState<ConversationMessage[]>([]);
  const [conversationError, setConversationError] = useState("");
  const [citation, setCitation] = useState<CitationPassage | null>(null);
  const [retainedImage, setRetainedImage] = useState<RetainedImage | null>(null);
  const [imageLoading, setImageLoading] = useState(false);
  const [imageError, setImageError] = useState("");
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
  const [ocrSource, setOcrSource] = useState<SourceSummary | null>(null);
  const [ocrExecutable, setOcrExecutable] = useState("/usr/bin/tesseract");
  const [ocrPreprocessor, setOcrPreprocessor] = useState("/usr/bin/magick");
  const [ocrEnhance, setOcrEnhance] = useState(true);
  const [ocrLanguage, setOcrLanguage] = useState("eng");
  const [ocrPageSegmentationMode, setOcrPageSegmentationMode] = useState("auto");
  const [ocrError, setOcrError] = useState("");
  const [ocrRunning, setOcrRunning] = useState(false);
  const [modelOpen, setModelOpen] = useState(false);
  const [modelProvider, setModelProvider] = useState<"ollama" | "llama-server">("ollama");
  const [modelEndpoint, setModelEndpoint] = useState("http://127.0.0.1:11434");
  const [ollamaModel, setOllamaModel] = useState("");
  const [modelApiKey, setModelApiKey] = useState("");
  const [modelError, setModelError] = useState("");
  const [modelRunning, setModelRunning] = useState(false);
  const [hybridOpen, setHybridOpen] = useState(false);
  const [hybridExecutable, setHybridExecutable] = useState("");
  const [hybridEndpoint, setHybridEndpoint] = useState("http://127.0.0.1:11434");
  const [hybridModel, setHybridModel] = useState("nomic-embed-text");
  const [hybridError, setHybridError] = useState("");
  const [hybridRunning, setHybridRunning] = useState(false);
  const [listening, setListening] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(() => matchMedia("(prefers-reduced-motion: reduce)").matches);
  const [dismissedTaskIds, setDismissedTaskIds] = useState<Set<string>>(() => new Set());
  const tasks = useMemo(() => Object.values(events.reduce<Record<string, TaskEvent>>((latest, event) => ({ ...latest, [event.task_id]: event }), {})).filter((task) => !dismissedTaskIds.has(task.task_id)).sort((a, b) => b.sequence - a.sequence), [dismissedTaskIds, events]);
  const [clock, setClock] = useState(() => Date.now());
  const [expandedTaskId, setExpandedTaskId] = useState<string | null>(null);
  const [expressionDebugEnabled, setExpressionDebugEnabled] = useState(false);
  const [debugExpression, setDebugExpression] = useState<EntityState | null>(null);
  const liveEntityState = deriveEntityState(tasks, listening);
  const entityState = debugExpression ?? liveEntityState;
  useEffect(() => {
    if (!tasks.length) return;
    const timer = window.setInterval(() => setClock(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [tasks.length]);
  useEffect(() => {
    if (!submittedPrompt || activeView !== "conversation") return;
    const frame = window.requestAnimationFrame(() => conversationRef.current?.scrollTo({ top: conversationRef.current.scrollHeight, behavior: reducedMotion ? "auto" : "smooth" }));
    return () => window.cancelAnimationFrame(frame);
  }, [activeView, reducedMotion, submittedPrompt]);
  useEffect(() => {
    if (!submittedPrompt) return;
    const latestUserMessage = [...conversationMessages].reverse().find((message) => message.role === "user");
    if (latestUserMessage?.content === submittedPrompt.text) setSubmittedPrompt(null);
  }, [conversationMessages, submittedPrompt]);
  const loadConversation = async (conversationId: string, expectedAskRun?: number): Promise<boolean> => {
    if (!IS_TAURI) return false;
    if (expectedAskRun !== undefined && expectedAskRun !== askRun.current) return false;
    setConversationError("");
    try {
      const detail = await invoke<ConversationDetail>("get_conversation", { conversationId });
      if (expectedAskRun !== undefined && expectedAskRun !== askRun.current) return false;
      setActiveConversationId(conversationId);
      setConversationMessages(detail.messages);
      return true;
    } catch (error) {
      if (expectedAskRun === undefined || expectedAskRun === askRun.current) setConversationError(String(error));
      return false;
    }
  };
  const refreshConversations = async (preferredId?: string, expectedAskRun?: number): Promise<boolean> => {
    if (!IS_TAURI) return false;
    try {
      const list = await invoke<ConversationSummary[]>("list_conversations");
      if (expectedAskRun !== undefined && expectedAskRun !== askRun.current) return false;
      setConversations(list);
      const next = preferredId || activeConversationId || list[0]?.id;
      if (next && list.some((conversation) => conversation.id === next)) return loadConversation(next, expectedAskRun);
      setActiveConversationId(null);
      setConversationMessages([]);
      return true;
    } catch (error) {
      if (expectedAskRun === undefined || expectedAskRun === askRun.current) setConversationError(String(error));
      return false;
    }
  };
  const refreshAssetCount = async () => {
    if (!IS_TAURI) return;
    const response = await invoke<AssetListResponse>("list_assets", { query: { limit: 1 } }).catch(() => null);
    if (response) setAssetCount(response.total);
  };
  const openAssets = () => {
    invalidateAskView();
    setActiveView("assets");
    setCitation(null);
    setRetainedImage(null);
    setImageError("");
  };
  const openConversation = () => {
    setActiveView("conversation");
  };

  useEffect(() => {
    if (IS_TAURI) void invoke<boolean>("expression_debug_enabled").then(setExpressionDebugEnabled).catch(() => setExpressionDebugEnabled(false));
    if (IS_TAURI) void invoke<RuntimeStatus>("runtime_status").then((runtime) => {
      setStatus(runtime);
      if (runtime.vault_mounted) {
        void invoke<SourceSummary[]>("list_sources").then(setSources).catch(() => undefined);
        void refreshAssetCount();
        void invoke<ConversationSummary[]>("list_conversations").then((list) => {
          setConversations(list);
          if (list[0]) void loadConversation(list[0].id);
        }).catch(() => undefined);
      }
    }).catch(() => setStatus(EMPTY_STATUS));
    if (IS_TAURI) void invoke<TaskEvent[]>("task_snapshot").then((snapshot) => setEvents((current) => mergeTaskEvents(current, snapshot))).catch(() => undefined);
    const unlisten = IS_TAURI
      ? listen<TaskEvent>("pinky://task-event", ({ payload }) => {
        setEvents((current) => mergeTaskEvents(current, [payload]));
        if (payload.state === "failed" || payload.state === "failed_interrupted") setExpandedTaskId(payload.task_id);
        if (payload.state === "completed" && payload.phase.name === "local ingestion") {
          void invoke<SourceSummary[]>("list_sources").then(setSources).catch(() => undefined);
          setAssetRefreshKey((current) => current + 1);
          void refreshAssetCount();
        }
        if (["completed", "failed", "cancelled"].includes(payload.state) && payload.phase.name === "image OCR") {
          if (payload.state === "completed") {
            void invoke<SourceSummary[]>("list_sources").then(setSources).catch(() => undefined);
            setAssetRefreshKey((current) => current + 1);
            void refreshAssetCount();
          }
          setOcrRunning(false);
        }
        if (payload.state === "completed" && ["delete image OCR", "delete OCR detection"].includes(payload.phase.name)) {
          void invoke<SourceSummary[]>("list_sources").then(setSources).catch(() => undefined);
          setAssetRefreshKey((current) => current + 1);
          void refreshAssetCount();
        }
        if (payload.state === "completed" && payload.phase.name === "cited answer") {
          void refreshConversations();
        }
        if (["completed", "failed", "cancelled"].includes(payload.state) && payload.phase.name === "model attach") {
          void invoke<RuntimeStatus>("runtime_status").then(setStatus).catch(() => undefined);
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
    void invoke<SourceSummary[]>("list_sources").then(setSources).catch(() => undefined);
    void refreshAssetCount();
    void refreshConversations();
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
  const dismissTask = (taskId: string) => {
    setDismissedTaskIds((current) => new Set(current).add(taskId));
    setExpandedTaskId((current) => current === taskId ? null : current);
  };
  const clearFinishedTasks = () => {
    setDismissedTaskIds((current) => {
      const next = new Set(current);
      tasks.filter(isFinishedTask).forEach((task) => next.add(task.task_id));
      return next;
    });
    setExpandedTaskId(null);
  };
  const askEnabled = canAsk(status, message, sources.length);
  const invalidateAskView = () => {
    askRun.current += 1;
    setAsking(false);
    setSubmittedPrompt(null);
  };
  const selectConversation = (conversationId: string) => {
    invalidateAskView();
    setMessage("");
    setAnswer(null);
    setSearchHits([]);
    setAskError("");
    setSearchError("");
    setConversationError("");
    setCitation(null);
    void loadConversation(conversationId);
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const query = message.trim();
    if (!query || !IS_TAURI || !status.vault_mounted) return;
    if (mode === "ask" && !askEnabled) return;
    setSubmittedPrompt({ text: query, submittedAt: new Date().toISOString() });
    setMessage("");
    setSearchError(""); setAskError("");
    if (mode === "ask") {
      const run = ++askRun.current;
      setAnswer(null); setAsking(true);
      try {
        const result = await invoke<AskQuestionResponse>("ask_question", { request: { question: query, conversation_id: activeConversationId } });
        if (run === askRun.current) {
          setAnswer(result.answer);
          await refreshConversations(result.conversation_id, run);
        }
      } catch (error) {
        if (run === askRun.current) {
          setAskError(String(error));
          setAsking(false);
          // The backend records the user message before inference begins. Reload
          // the conversation so a failed first request can still be continued
          // instead of leaving the newly-created chat orphaned from the UI.
          await refreshConversations(undefined, run);
          setSubmittedPrompt(null);
        }
      } finally {
        if (run === askRun.current) setAsking(false);
      }
      return;
    }
    // Search is additive: keep the current conversation and any validated
    // answer visible while matching evidence is loaded below it.
    setSearching(true);
    try {
      setSearchHits(await invoke<SearchHit[]>("search_sources", { query, limit: 8 }));
    } catch (error) { setSearchError(String(error)); }
    finally { setSearching(false); }
  };
  const showCitation = async (uri: string) => {
    if (!IS_TAURI) return;
    setSearchError("");
    setRetainedImage(null); setImageError("");
    try { setCitation(await invoke<CitationPassage>("open_citation", { citationUri: uri })); }
    catch (error) { setSearchError(String(error)); }
  };
  const showRetainedImage = async () => {
    if (!IS_TAURI || !citation || !citation.mime_type.startsWith("image/")) return;
    setImageError(""); setImageLoading(true);
    try { setRetainedImage(await invoke<RetainedImage>("open_retained_image", { citationUri: citation.citation_uri })); }
    catch (error) { setImageError(String(error)); }
    finally { setImageLoading(false); }
  };
  const openOcr = (source: SourceSummary) => { setOcrSource(source); setOcrExecutable("/usr/bin/tesseract"); setOcrPreprocessor("/usr/bin/magick"); setOcrEnhance(true); setOcrLanguage("eng"); setOcrPageSegmentationMode("auto"); setOcrError(""); };
  const runOcr = async (event: FormEvent) => {
    event.preventDefault();
    if (!IS_TAURI || !ocrSource) return;
    setOcrError(""); setOcrRunning(true);
    try {
      const automatic = ocrPageSegmentationMode === "auto";
      await invoke<string>("ocr_image", { request: { source_id: ocrSource.source_id, version_id: ocrSource.version_id, executable: ocrExecutable, language: ocrLanguage, preprocessor: ocrEnhance ? ocrPreprocessor : null, page_segmentation_mode: automatic ? null : Number(ocrPageSegmentationMode), auto_page_segmentation: automatic } });
      setOcrSource(null);
    } catch (error) { setOcrError(String(error)); setOcrRunning(false); }
  };
  const newConversation = async () => {
    if (!IS_TAURI || !status.vault_mounted) return;
    invalidateAskView();
    setConversationError("");
    setAskError("");
    setSearchError("");
    setCitation(null); setRetainedImage(null); setImageError("");
    setAnswer(null);
    setSearchHits([]);
    setMessage("");
    setActiveConversationId(null);
    setConversationMessages([]);
    try {
      const created = await invoke<ConversationSummary>("create_conversation", { request: { title: "New conversation" } });
      setMode("ask");
      await refreshConversations(created.id);
    } catch (error) {
      setConversationError(String(error));
      await refreshConversations();
    }
  };
  const renameConversation = async (conversation: ConversationSummary) => {
    if (!IS_TAURI) return;
    const title = window.prompt("Conversation title", conversation.title)?.trim();
    if (!title || title === conversation.title) return;
    try {
      await invoke("rename_conversation", { conversationId: conversation.id, request: { title } });
      await refreshConversations(conversation.id);
    } catch (error) { setConversationError(String(error)); }
  };
  const deleteConversation = async (conversation: ConversationSummary) => {
    if (!IS_TAURI || !window.confirm(`Delete “${conversation.title}”? Encrypted message references will be removed; backups may still contain them.`)) return;
    try {
      await invoke("delete_conversation", { conversationId: conversation.id });
      const remaining = conversations.filter((item) => item.id !== conversation.id);
      setConversations(remaining);
      if (activeConversationId === conversation.id) {
        setActiveConversationId(null); setConversationMessages([]); setAnswer(null);
        if (remaining[0]) await loadConversation(remaining[0].id);
      }
    } catch (error) { setConversationError(String(error)); }
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
      await refreshAssetCount();
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
      if (refreshed.vault_mounted) {
        setSources(await invoke<SourceSummary[]>("list_sources").catch(() => []));
        await refreshAssetCount();
      }
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
  const openModel = () => { setModelApiKey(""); setModelError(""); setModelOpen(true); };
  const closeModel = () => {
    if (modelRunning) return;
    setModelApiKey(""); setModelError(""); setModelOpen(false);
  };
  const attachModel = async (event: FormEvent) => {
    event.preventDefault(); setModelError("");
    if (!IS_TAURI) { setModelError("Local model attachment is available only in the native application."); return; }
    setModelRunning(true);
    try {
      if (modelProvider === "ollama") {
        await invoke<AttachLlamaResponse>("attach_ollama", { request: { endpoint: modelEndpoint.trim(), model: ollamaModel.trim() } });
      } else {
        await invoke<AttachLlamaResponse>("attach_llama_server", { request: { endpoint: modelEndpoint.trim(), api_key: modelApiKey } });
      }
      setModelApiKey("");
      setStatus(await invoke<RuntimeStatus>("runtime_status"));
    } catch (error) { setModelError(String(error)); }
    finally { setModelRunning(false); }
  };
  const detachModel = async () => {
    if (!IS_TAURI) return;
    setModelError("");
    try {
      await invoke("detach_local_model");
      setStatus(await invoke<RuntimeStatus>("runtime_status"));
      closeModel();
    } catch (error) { setModelError(String(error)); }
  };
  const openHybrid = async () => {
    setHybridError(""); setHybridOpen(true);
    if (!IS_TAURI) return;
    const configuration = await invoke<HybridConfiguration | null>("get_hybrid_configuration").catch(() => null);
    if (configuration) {
      setHybridExecutable(configuration.qdrant_executable);
      setHybridEndpoint(configuration.embedding_endpoint);
      setHybridModel(configuration.embedding_model);
    }
  };
  const closeHybrid = () => { if (!hybridRunning) { setHybridError(""); setHybridOpen(false); } };
  const configureHybrid = async (event: FormEvent) => {
    event.preventDefault(); setHybridError("");
    if (!IS_TAURI) { setHybridError("Hybrid retrieval configuration is available only in the native application."); return; }
    setHybridRunning(true);
    try {
      await invoke("configure_hybrid_retrieval", { request: { qdrant_executable: hybridExecutable.trim(), embedding_endpoint: hybridEndpoint.trim(), embedding_model: hybridModel.trim() } });
      setStatus(await invoke<RuntimeStatus>("runtime_status"));
      setHybridOpen(false);
    } catch (error) { setHybridError(String(error)); }
    finally { setHybridRunning(false); }
  };
  const clearHybrid = async () => {
    if (!IS_TAURI) return;
    setHybridError(""); setHybridRunning(true);
    try {
      await invoke("clear_hybrid_configuration");
      setStatus(await invoke<RuntimeStatus>("runtime_status"));
      setHybridOpen(false);
    } catch (error) { setHybridError(String(error)); }
    finally { setHybridRunning(false); }
  };
  const openAmbient = () => {
    if (IS_TAURI) void invoke("open_ambient_window").catch(() => undefined);
  };

  return <main className="app-shell">
    <a className="skip-link" href="#conversation">Skip to conversation</a>
    <aside className="left-panel" aria-label="Knowledge navigation">
      <header className="brand"><span className="brand-mark">P</span><div><strong>PINKY</strong><small>PRIVATE INTELLIGENCE</small></div></header>
      <button className="new-chat" disabled={!status.vault_mounted} onClick={() => void newConversation()} title={status.vault_mounted ? "Create an encrypted conversation" : "Unlock the vault before creating a conversation"}><Plus size={15} /> New conversation</button>
        <nav>
        <NavGroup icon={<MessageSquare />} label="Chats" count={String(conversations.length)}>{conversations.length ? conversations.map((conversation) => <div className={`chat-row ${conversation.id === activeConversationId ? "active" : ""}`} key={conversation.id}><button className="chat-select" onClick={() => { setMode("ask"); selectConversation(conversation.id); }}><MessageSquare size={12} /><span>{conversation.title}</span></button><button className="chat-action" aria-label={`Rename ${conversation.title}`} title="Rename conversation" onClick={() => void renameConversation(conversation)}><Pencil size={11} /></button><button className="chat-action danger" aria-label={`Delete ${conversation.title}`} title="Delete conversation" onClick={() => void deleteConversation(conversation)}><Trash2 size={11} /></button></div>) : <p className="empty-nav">No conversations yet</p>}{conversationError && <p className="nav-error" role="alert">{conversationError}</p>}</NavGroup>
        <NavGroup icon={<Database />} label="Assets" count={status.vault_mounted ? String(assetCount) : "—"}><button className={`nav-row asset-nav-row${activeView === "assets" ? " active" : ""}`} disabled={!status.vault_mounted} onClick={openAssets}><Database size={13} /> Open asset library</button><button className="nav-row" disabled={!status.vault_mounted} onClick={openSource}><Plus size={13} /> Add source</button></NavGroup>
        <NavGroup icon={<BookOpen />} label="Dossiers" count="0" />
        <NavGroup icon={<FolderKey />} label="Workspaces" count="0"><button className="nav-row"><Plus size={13} /> Approve directory</button></NavGroup>
      </nav>
      <div className={`vault-card ${status.vault_mounted ? "ready" : "locked"}`}><ShieldCheck size={17} /><div><strong>{status.vault_mounted ? "Vault unlocked" : status.vault_registered ? "Vault registered" : "Vault locked"}</strong><small>{status.vault_mounted ? "Encrypted storage available" : status.vault_registered ? "Waiting for encrypted storage to unlock" : "Setup required before data can be retained"}</small></div></div>
    </aside>

    <section className="centre-panel" id="conversation" aria-label="Conversation">
      <div className="topbar"><div className="crumb">Retained knowledge <ChevronRight size={13} /> <span>{activeView === "assets" ? "Asset library" : mode === "ask" ? "Cited answer" : "Lexical search"}</span></div><div className="topbar-actions"><button type="button" className="ambient-launcher" onClick={openAmbient} disabled={!IS_TAURI} title="Open Ambient ALMA"><Sparkles size={13} /> Ambient</button>{activeView === "assets" ? <button type="button" className="asset-back" onClick={openConversation}><ArrowLeft size={13} /> Back to chat</button> : <div className="mode-switch" role="group" aria-label="Conversation mode"><button className={mode === "ask" ? "active" : ""} onClick={() => { invalidateAskView(); setMode("ask"); setSearchHits([]); setSearchError(""); }} aria-pressed={mode === "ask"}>Ask</button><button className={mode === "search" ? "active" : ""} onClick={() => { invalidateAskView(); setMode("search"); setAskError(""); }} aria-pressed={mode === "search"}><Search size={13} /> Search</button></div>}</div></div>
      {activeView === "assets" ? <AssetLibrary refreshKey={assetRefreshKey} vaultMounted={status.vault_mounted} onAddSource={openSource} onOpenCitation={(uri) => void showCitation(uri)} onOpenOcr={openOcr} /> : <>
      <div className="conversation" ref={conversationRef} role="region" aria-label="Conversation and search results" tabIndex={0}>
        <div className="entity-stage"><AsciiEntity state={entityState} reducedMotion={reducedMotion} /><span className={`state-pill ${entityState}`} aria-live="polite"><i /> ALMA · {entityState}</span></div>
        {expressionDebugEnabled && <details className="expression-lab" open={debugExpression !== null}>
          <summary>Temporary expression lab{debugExpression ? ` · ${debugExpression}` : ""}</summary>
          <div className="expression-lab-body">
            <p>Preview each ALMA state without starting a real task.</p>
            <div className="expression-grid" role="group" aria-label="ALMA expression preview">
              {ENTITY_STATES.map((expression) => <button type="button" key={expression} className={entityState === expression && debugExpression ? "active" : ""} onClick={() => setDebugExpression(expression)} aria-pressed={entityState === expression && debugExpression !== null}>{expression}</button>)}
            </div>
            <button type="button" className="expression-live" onClick={() => setDebugExpression(null)} disabled={debugExpression === null}>Return to live task state</button>
          </div>
        </details>}
        <div className="welcome"><p className="eyebrow">ENCRYPTED · LOCAL · SOURCE-GROUNDED</p><h1>What should we understand<br />or create?</h1><p>Pinky retains approved evidence inside your encrypted vault and shows every operation while it works.</p></div>
        {(conversationMessages.length > 0 || submittedPrompt) && <ConversationHistory messages={answer ? conversationMessages.slice(0, -1) : conversationMessages} pendingMessage={submittedPrompt} onCitation={(uri) => void showCitation(uri)} />}
        {!status.vault_mounted && (status.vault_registered ? <div className="blocking-question" role="alert"><KeyRound size={19} /><div><strong>{status.setup_in_progress ? "Unlocking your encrypted vault" : "Your registered vault is locked"}</strong><p>{status.unlock_error || "Pinky is retrieving its protected key from Linux Secret Service."}</p></div><button disabled={status.setup_in_progress || !status.prerequisites.gocryptfs || !status.prerequisites.secret_service} onClick={retryUnlock}>{status.setup_in_progress ? "Unlocking…" : "Retry unlock"}</button></div> : <div className="blocking-question" role="alert"><FolderKey size={19} /><div><strong>Set up the encrypted vault to begin</strong><p>Ingestion, conversations, generation, and logs stay disabled until gocryptfs and Secret Service are ready.</p></div><button onClick={openSetup}>Start setup</button></div>)}
        {(searchError || askError) && <div className="error-panel" role="alert"><p className="search-error">{searchError || askError}</p><div className="error-actions">{askError && isReconnectableError(askError) && <button className="reconnect-button" onClick={openModel}><Cpu size={13} /> Reconnect local model</button>}{askError && <button className="reconnect-button" onClick={() => void newConversation()}>New conversation</button>}<button className="reconnect-button" onClick={() => { setSearchError(""); setAskError(""); }}>Dismiss</button></div></div>}
        {answer && <AnswerView answer={answer} onCitation={(uri) => void showCitation(uri)} />}
        {!!searchHits.length && <section className="search-results" aria-label="Retained source search results"><header><p className="eyebrow">MATCHING EVIDENCE</p><span>{searchHits.length} passage{searchHits.length === 1 ? "" : "s"}</span></header>{searchHits.map((hit) => <article key={hit.chunk_id}><div><FileText size={14} /><strong>{hit.display_name}</strong>{hit.heading && <span>{hit.heading}</span>}</div><p>{hit.passage}</p><button onClick={() => void showCitation(hit.citation_uri)}>{formatCoordinates(hit.coordinates)} · Open retained citation</button></article>)}</section>}
      </div>
      <form className="composer" onSubmit={submit}>
        <textarea aria-label={mode === "ask" ? "Ask a cited question" : "Search retained sources"} value={message} onChange={(event) => setMessage(event.target.value)} onFocus={() => setListening(true)} onBlur={() => setListening(false)} placeholder={status.vault_mounted ? (mode === "ask" ? "Ask about your retained evidence…" : "Search your retained sources…") : "Unlock the encrypted vault to start…"} disabled={!status.vault_mounted || searching || asking} onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); event.currentTarget.form?.requestSubmit(); } }} />
        <div className="composer-footer"><div><button type="button" className="tool-chip" disabled={!status.vault_mounted} onClick={openSource}><Plus size={14} /> Attach source</button><span>{status.vault_mounted ? (mode === "ask" ? "Local inference · validated citations" : "Lexical retrieval · exact citations") : "Encrypted vault required"}</span></div><button className="send" aria-label={mode === "ask" ? "Ask question" : "Search"} disabled={!status.vault_mounted || searching || asking || !message.trim() || (mode === "ask" && !askEnabled)}>{searching || asking ? "…" : "↑"}</button></div>
      </form>
      </>}
    </section>

    <aside className="right-panel" aria-label="Task activity">
      <div className="task-header"><div><p className="eyebrow">OPERATIONS</p><h2>Tasks</h2></div><div className="task-header-actions"><button className="clear-finished" disabled={!tasks.some(isFinishedTask)} onClick={clearFinishedTasks}><Trash2 size={13} /> Clear finished</button><button className="stop-all" disabled={!tasks.some((task) => task.cancellable)} onClick={stopAll}><OctagonX size={14} /> Stop all</button></div></div>
      <div className="task-list">
        {!tasks.length && <div className="empty-tasks"><Box size={30} /><strong>No active operations</strong><p>Ingestion, research, generation, and tool work will appear here.</p><button onClick={startCheck}><Play size={13} /> Run system check</button></div>}
        {tasks.map((task) => <article className={`task-card ${task.state}${expandedTaskId === task.task_id ? " expanded" : ""}`} key={task.task_id}>
          <div className="task-title"><i className={`task-state-dot ${task.state}`} title={`Task state: ${task.state.replaceAll("_", " ")}`} aria-label={`Task state: ${task.state.replaceAll("_", " ")}`} /><div><strong>{task.phase.name}</strong>{task.phase.activity.toLowerCase() !== task.state.replaceAll("_", " ").toLowerCase() && <span className="task-activity">{task.phase.activity}</span>}</div><span>{task.phase.progress == null ? "—" : `${Math.round(task.phase.progress * 100)}%`}</span></div>
          <div className={`progress${task.phase.progress == null ? " indeterminate" : ""}`}><span style={task.phase.progress == null ? undefined : { width: `${task.phase.progress * 100}%` }} /></div>
          <div className="task-meta"><span title={`Permission: ${task.permission_state}`} aria-label={`Permission: ${task.permission_state}`}>{task.permission_state}</span><span title={`Budget: ${task.budget_state.replaceAll("_", " ")}`} aria-label={`Budget: ${task.budget_state.replaceAll("_", " ")}`}>{task.budget_state.replaceAll("_", " ")}</span><span title={task.timestamp}>{taskAge(task.timestamp, clock)}</span></div>
          <div className="task-card-controls"><button type="button" className="task-inspect" onClick={() => setExpandedTaskId((current) => current === task.task_id ? null : task.task_id)} aria-expanded={expandedTaskId === task.task_id} aria-controls={`task-detail-${task.task_id}`}>{expandedTaskId === task.task_id ? "Hide details" : "Inspect task"}</button>{isFinishedTask(task) && <button type="button" className="task-dismiss" onClick={() => dismissTask(task.task_id)}><X size={11} /> Dismiss</button>}</div>
          {task.cancellable && <div className="task-actions"><button onClick={() => pause(task.task_id, task.phase.activity !== "Paused")}><CirclePause size={13} /> {task.phase.activity === "Paused" ? "Resume" : "Pause"}</button><button onClick={() => stop(task.task_id)}><Square size={12} /> Stop</button></div>}
          {task.error && <p className="task-error">{task.error.message}</p>}
          {expandedTaskId === task.task_id && <TaskDetail task={task} events={events.filter((event) => event.task_id === task.task_id)} />}
        </article>)}
      </div>
      <section className="runtime"><p className="eyebrow">RUNTIME</p>{Object.entries(status.prerequisites).map(([name, available]) => <div key={name}><span>{name.replace("_", " ")}</span><b className={available ? "ok" : "missing"}>{available ? "ready" : "missing"}</b></div>)}<div><span>task journal</span><b className={status.task_journal_error ? "missing" : "ok"} title={status.task_journal_error || undefined}>{status.task_journal_error ? "error" : status.vault_mounted ? "durable" : "locked"}</b></div><div><span>file watcher</span><b className={status.watcher_error ? "missing" : "ok"} title={status.watcher_error || undefined}>{status.watcher_error ? "error" : status.vault_mounted ? "watching" : "locked"}</b></div><div><span>local model</span><b className={status.model_connected ? "ok" : status.model_error ? "missing" : ""} title={status.model_error || undefined}>{status.model_attach_in_progress ? "checking" : status.model_connected ? "attached" : status.model_error ? "error" : "offline"}</b></div><div><span>hybrid retrieval</span><b className={status.hybrid_configured ? "ok" : ""}>{status.hybrid_configured ? status.hybrid_model || "configured" : "lexical only"}</b></div>{status.vault_mounted && !sources.length && <div className="runtime-hint" role="status"><Database size={12} /><div><strong>Source search</strong><span>Add a retained source to search or ask.</span></div><button onClick={openSource}>Add</button></div>}{status.vault_mounted && mode === "ask" && sources.length > 0 && !status.model_connected && <div className="runtime-hint" role="status"><MessageSquare size={12} /><div><strong>Ask mode</strong><span>Attach a local model for cited answers.</span></div><button onClick={openModel}>Attach</button></div>}<button className="runtime-action" disabled={!status.vault_mounted || status.model_attach_in_progress} onClick={openModel}><Cpu size={12} /> {status.model_connected ? "Model details" : "Attach local model"}</button><button className="runtime-action" disabled={!status.vault_mounted || hybridRunning} onClick={openHybrid}><Settings2 size={12} /> {status.hybrid_configured ? "Hybrid retrieval settings" : "Configure hybrid retrieval"}</button></section>
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
    {modelOpen && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) closeModel(); }}>
      <section className="setup-modal model-modal" role="dialog" aria-modal="true" aria-labelledby="model-title">
        <header><div><p className="eyebrow">LOCAL INFERENCE</p><h2 id="model-title">Attach local model</h2></div><button aria-label="Close model connection" disabled={modelRunning} onClick={closeModel}><X size={17} /></button></header>
        {status.model_connected ? <div className="setup-complete"><Cpu size={34} /><h3>{status.model_name}</h3><p>{status.model_provider} model attached with {status.model_context_size?.toLocaleString()} context tokens.</p><p>{status.model_provider === "Ollama" ? "The local Ollama API does not require or store an API key." : "The API key exists only in this Pinky process."} Ask mode will retrieve and validate citations before rendering an answer.</p>{modelError && <p className="setup-error" role="alert">{modelError}</p>}<button className="disconnect-model" onClick={() => void detachModel()}>Detach model</button></div> : <form onSubmit={attachModel}>
          <div className="setup-intro"><Cpu size={21} /><p>{modelProvider === "ollama" ? "Connect to an already-running local Ollama instance without an API key." : "Connect to an already-running llama.cpp server using its ephemeral API key."} Pinky accepts only an IPv4 loopback endpoint.</p></div>
          <label>Provider<select value={modelProvider} onChange={(event) => { const provider = event.target.value as "ollama" | "llama-server"; setModelProvider(provider); setModelEndpoint(provider === "ollama" ? "http://127.0.0.1:11434" : "http://127.0.0.1:8080"); setModelApiKey(""); setModelError(""); }}><option value="ollama">Ollama (no key)</option><option value="llama-server">llama-server (API key)</option></select></label>
          <label>Server endpoint<input type="url" value={modelEndpoint} onChange={(event) => setModelEndpoint(event.target.value)} placeholder={modelProvider === "ollama" ? "http://127.0.0.1:11434" : "http://127.0.0.1:8080"} spellCheck={false} required /></label>
          {modelProvider === "ollama" ? <label>Installed model name<input value={ollamaModel} onChange={(event) => setOllamaModel(event.target.value)} placeholder="qwen3:8b" spellCheck={false} required /></label> : <label>256-bit API key<input type="password" autoComplete="off" value={modelApiKey} onChange={(event) => setModelApiKey(event.target.value)} minLength={64} maxLength={64} pattern="[0-9A-Fa-f]{64}" spellCheck={false} required /></label>}
          <p className="source-support">{modelProvider === "ollama" ? <>Enter a model shown by <code>ollama list</code>. Pinky verifies it through <code>/api/tags</code> and <code>/api/show</code>; remote/cloud proxies are rejected, and only a local GGUF completion model with at least 2,048 context tokens is accepted.</> : <>The server must use the same 64-character hexadecimal token, expose <code>/health</code> and <code>/props</code>, and provide at least 2,048 context tokens.</>}</p>
          {(modelError || status.model_error) && <p className="setup-error" role="alert">{modelError || status.model_error}</p>}
          <footer><button type="button" disabled={modelRunning} onClick={closeModel}>Cancel</button><button className="primary" disabled={modelRunning}>{modelRunning ? "Checking local model…" : "Attach model"}</button></footer>
        </form>}
      </section>
    </div>}
    {hybridOpen && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) closeHybrid(); }}>
      <section className="setup-modal model-modal" role="dialog" aria-modal="true" aria-labelledby="hybrid-title">
        <header><div><p className="eyebrow">HYBRID RETRIEVAL</p><h2 id="hybrid-title">Configure vector retrieval</h2></div><button aria-label="Close hybrid retrieval settings" disabled={hybridRunning} onClick={closeHybrid}><X size={17} /></button></header>
        <form onSubmit={configureHybrid}>
          <div className="setup-intro"><Settings2 size={21} /><p>Pinky stores these settings inside the encrypted vault, validates the local embedding model, and verifies a supervised Qdrant process before enabling hybrid retrieval.</p></div>
          <label>Qdrant executable<input value={hybridExecutable} onChange={(event) => setHybridExecutable(event.target.value)} placeholder="/usr/local/bin/qdrant" spellCheck={false} required /></label>
          <label>Embedding Ollama endpoint<input type="url" value={hybridEndpoint} onChange={(event) => setHybridEndpoint(event.target.value)} placeholder="http://127.0.0.1:11434" spellCheck={false} required /></label>
          <label>Installed embedding model<input value={hybridModel} onChange={(event) => setHybridModel(event.target.value)} placeholder="nomic-embed-text" spellCheck={false} required /></label>
          <p className="source-support">The endpoint must be an explicit IPv4 loopback address. The embedding model must be installed locally, expose the embedding capability, and return a stable vector dimension. Qdrant data remains inside the mounted encrypted vault.</p>
          {hybridError && <p className="setup-error" role="alert">{hybridError}</p>}
          <footer><button type="button" disabled={hybridRunning} onClick={closeHybrid}>Cancel</button>{status.hybrid_configured && <button type="button" disabled={hybridRunning} onClick={() => void clearHybrid()}>Disable hybrid</button>}<button className="primary" disabled={hybridRunning}>{hybridRunning ? "Verifying hybrid runtime…" : "Verify and enable"}</button></footer>
        </form>
      </section>
    </div>}
    {sourceOpen && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) closeSource(); }}>
      <section className="setup-modal source-modal" role="dialog" aria-modal="true" aria-labelledby="source-title">
        <header><div><p className="eyebrow">APPROVED LOCAL SOURCE</p><h2 id="source-title">Retain a local source</h2></div><button aria-label="Close source setup" onClick={closeSource}><X size={17} /></button></header>
        <form onSubmit={runIngestion}>
          <div className="setup-intro"><FolderKey size={21} /><p>The approved root is the directory Pinky may access. The source must resolve inside it; devices, sockets, directories, and symlink escapes are rejected.</p></div>
          <label>Approved directory<input value={approvedRoot} onChange={(event) => setApprovedRoot(event.target.value)} placeholder="/home/you/Documents" spellCheck={false} required /></label>
          <label>Source file<input value={sourcePath} onChange={(event) => setSourcePath(event.target.value)} placeholder="/home/you/Documents/notes.md" spellCheck={false} required /></label>
          <p className="source-support">Currently searchable: UTF-8 text, Markdown, logs, source code, JSON, YAML, XML, HTML, CSV, and image metadata for PNG, JPEG, WebP, GIF, and TIFF. Image rows can run a bounded local OCR worker and citations can open the exact retained pixels. Other formats are safely archived and marked unsupported.</p>
          {sourceError && <p className="setup-error" role="alert">{sourceError}</p>}
          <footer><button type="button" onClick={closeSource}>Cancel</button><button className="primary">Archive and extract</button></footer>
        </form>
      </section>
    </div>}
    {ocrSource && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !ocrRunning) setOcrSource(null); }}>
      <section className="setup-modal source-modal" role="dialog" aria-modal="true" aria-labelledby="ocr-title">
        <header><div><p className="eyebrow">RETAINED IMAGE</p><h2 id="ocr-title">Extract text with local OCR</h2></div><button aria-label="Close OCR setup" disabled={ocrRunning} onClick={() => setOcrSource(null)}><X size={17} /></button></header>
        <form onSubmit={runOcr}>
          <div className="setup-intro"><FileText size={21} /><p>The worker receives only the retained image, runs as a supervised process, and writes temporary input/output under the mounted vault. Each OCR run is retained as encrypted searchable chunks, so you can retry with a different layout mode without losing earlier citations.</p></div>
          <p className="source-support"><strong>{ocrSource.display_name}</strong> · {ocrSource.mime_type} · version {ocrSource.version_id}</p>
          <label>OCR executable<input value={ocrExecutable} onChange={(event) => setOcrExecutable(event.target.value)} placeholder="/usr/bin/tesseract" spellCheck={false} required /></label>
          <label>Language code<input value={ocrLanguage} onChange={(event) => setOcrLanguage(event.target.value)} placeholder="eng" spellCheck={false} required /></label>
          <label className="ocr-check"><input type="checkbox" checked={ocrEnhance} onChange={(event) => setOcrEnhance(event.target.checked)} /> Enhance photographed/scanned images before OCR</label>
          {ocrEnhance && <><label>ImageMagick executable<input value={ocrPreprocessor} onChange={(event) => setOcrPreprocessor(event.target.value)} placeholder="/usr/bin/magick" spellCheck={false} required /></label><p className="source-support">Enhancement auto-orients, deskews, converts to grayscale, upscales 2×, stretches contrast, and sharpens before Tesseract runs. The processed pixels remain temporary inside the encrypted vault.</p></>}
          <label>Page segmentation mode<select value={ocrPageSegmentationMode} onChange={(event) => setOcrPageSegmentationMode(event.target.value)}><option value="auto">Try all layouts automatically</option><option value="3">3 · Automatic page layout</option><option value="4">4 · Single column</option><option value="6">6 · Uniform text block</option><option value="7">7 · Single text line</option><option value="8">8 · Single word</option><option value="11">11 · Sparse text</option><option value="12">12 · Sparse text with orientation detection</option><option value="13">13 · Raw text line</option></select></label>
          <p className="source-support">Automatic mode tries every Tesseract layout mode (0–13) at minimum confidence thresholds of 55%, 45%, 35%, and 25%, stopping at the first accepted non-empty result. That is up to 56 supervised attempts; the task reports each combination and fails clearly if they are all exhausted.</p>
          <p className="source-support">Install Tesseract separately if this path does not exist. Pinky does not download OCR runtimes or send pixels to a remote service.</p>
          {ocrError && <p className="setup-error" role="alert">{ocrError}</p>}
          <footer><button type="button" disabled={ocrRunning} onClick={() => setOcrSource(null)}>Cancel</button><button className="primary" disabled={ocrRunning}>{ocrRunning ? "Running supervised OCR…" : "Run / rerun OCR"}</button></footer>
        </form>
      </section>
    </div>}
    {citation && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) setCitation(null); }}>
      <section className="setup-modal citation-modal" role="dialog" aria-modal="true" aria-labelledby="citation-title">
        <header><div><p className="eyebrow">RETAINED SOURCE SNAPSHOT</p><h2 id="citation-title">{citation.display_name}</h2></div><button aria-label="Close citation" onClick={() => setCitation(null)}><X size={17} /></button></header>
        <div className="citation-provenance"><span>{citation.mime_type}</span><span>{formatCoordinates(citation.coordinates)}</span><span>Retrieved {new Date(citation.retrieved_at).toLocaleString()}</span></div>
        {citation.mime_type.startsWith("image/") && <div className="image-viewer"><button type="button" className="primary" disabled={imageLoading} onClick={() => void showRetainedImage()}>{imageLoading ? "Loading retained pixels…" : retainedImage ? "Reload retained pixels" : "View retained pixels"}</button>{imageError && <p className="setup-error" role="alert">{imageError}</p>}{retainedImage && <img className="retained-image" src={`data:${retainedImage.mime_type};base64,${retainedImage.bytes_base64}`} alt={`Retained ${retainedImage.display_name}`} />}</div>}
        <pre>{citation.passage}</pre><code>{citation.citation_uri}</code><p className="citation-origin" title={citation.canonical_uri}>{citation.canonical_uri}</p>
      </section>
    </div>}
  </main>;
}

function formatCoordinates(coordinates: SearchHit["coordinates"]) {
  if (!coordinates?.line_start) return "Chunk passage";
  return coordinates.line_start === coordinates.line_end ? `Line ${coordinates.line_start}` : `Lines ${coordinates.line_start}–${coordinates.line_end}`;
}

function ConversationHistory({ messages, pendingMessage, onCitation }: { messages: ConversationMessage[]; pendingMessage?: { text: string; submittedAt: string } | null; onCitation: (uri: string) => void }) {
  return <section className="conversation-history" aria-label="Encrypted conversation history">
    {messages.map((message) => <article className={`message ${message.role}`} key={message.id}><div className="message-label">{message.role === "user" ? "You" : "ALMA"}<time dateTime={message.created_at}>{new Date(message.created_at).toLocaleString()}</time></div><p>{message.content}</p>{message.citations.length > 0 && <CitationLinks citations={message.citations} onCitation={onCitation} label="Message sources" />}</article>)}
    {pendingMessage && <article className="message user pending"><div className="message-label">You<time dateTime={pendingMessage.submittedAt}>Sending now</time></div><p>{pendingMessage.text}</p></article>}
  </section>;
}

function AnswerView({ answer, onCitation }: { answer: AnswerEnvelope; onCitation: (uri: string) => void }) {
  const citations = [...new Set(answer.summary_citations)];
  return <section className="answer-view" aria-label="Cited answer" aria-live="polite">
    <header><p className="eyebrow">SOURCE-GROUNDED ANSWER</p><span>validated schema {answer.schema_version}</span></header>
    <p className="answer-summary">{answer.summary}</p>
    {!!citations.length && <CitationLinks citations={citations} onCitation={onCitation} label="Summary sources" />}
    {!!answer.claims.length && <div className="answer-claims">{answer.claims.map((claim, index) => <article key={`${claim.statement}-${index}`}><div className="claim-label"><strong>{claim.support}</strong><span>Claim {index + 1}</span></div><p>{claim.statement}</p><CitationLinks citations={claim.citations} onCitation={onCitation} label="Claim sources" /></article>)}</div>}
    {!!answer.warnings.length && <div className="answer-notes warning" role="note"><strong>Evidence warnings</strong><ul>{answer.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul></div>}
    {!!answer.unresolved_gaps.length && <div className="answer-notes gap" role="note"><strong>Unresolved gaps</strong><ul>{answer.unresolved_gaps.map((gap) => <li key={gap}>{gap}</li>)}</ul></div>}
  </section>;
}

function CitationLinks({ citations, onCitation, label }: { citations: string[]; onCitation: (uri: string) => void; label: string }) {
  return <div className="answer-citations" aria-label={label}>{citations.map((citation, index) => <button key={citation} title={citation} aria-label={`Open citation ${citation}`} onClick={() => onCitation(citation)}>{compactCitationLabel(citation, index)}</button>)}</div>;
}

function compactCitationLabel(citation: string, index: number) {
  const coordinate = citation.match(/^pinky:\/\/source\/[^/]+\/version\/[^#]+#(.+)$/)?.[1];
  const readableCoordinate = coordinate?.replace(/^chunk-/, "chunk ");
  return readableCoordinate ? `Source ${index + 1} · ${readableCoordinate}` : `Source ${index + 1}`;
}

function AssetLibrary({ refreshKey, vaultMounted, onAddSource, onOpenCitation, onOpenOcr }: { refreshKey: number; vaultMounted: boolean; onAddSource: () => void; onOpenCitation: (uri: string) => void; onOpenOcr: (source: SourceSummary) => void }) {
  const [items, setItems] = useState<SourceSummary[]>([]);
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [kind, setKind] = useState<"all" | SourceSummary["kind"]>("all");
  const [state, setState] = useState<"all" | SourceSummary["state"]>("all");
  const [sort, setSort] = useState<AssetSort>("updated");
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [total, setTotal] = useState(0);
  const [selected, setSelected] = useState<SourceSummary | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [view, setView] = useState<"list" | "grid">("list");
  const [manualRefresh, setManualRefresh] = useState(0);
  const [textSourceId, setTextSourceId] = useState<string | null>(null);
  const [textPassages, setTextPassages] = useState<CitationPassage[]>([]);
  const [textLoading, setTextLoading] = useState(false);
  const [textError, setTextError] = useState("");

  useEffect(() => {
    const timer = window.setTimeout(() => setDebouncedQuery(query.trim()), 250);
    return () => window.clearTimeout(timer);
  }, [query]);
  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      if (!IS_TAURI || !vaultMounted) {
        setItems([]);
        setTotal(0);
        setNextCursor(null);
        return;
      }
      setLoading(true);
      setError("");
      try {
        const request = {
          search: debouncedQuery || undefined,
          kind: kind === "all" ? undefined : kind,
          state: state === "all" ? undefined : state,
          sort,
          limit: 40,
        };
        const response = await invoke<AssetListResponse>("list_assets", { query: request });
        if (cancelled) return;
        setItems(response.items);
        setTotal(response.total);
        setNextCursor(response.next_cursor);
        setSelected((current) => current && response.items.some((item) => item.source_id === current.source_id) ? response.items.find((item) => item.source_id === current.source_id) || current : null);
      } catch (loadError) {
        if (!cancelled) setError(String(loadError));
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void load();
    return () => { cancelled = true; };
  }, [debouncedQuery, kind, state, sort, refreshKey, manualRefresh, vaultMounted]);
  useEffect(() => {
    if (refreshKey === 0) return;
    setTextSourceId(null);
    setTextPassages([]);
    setTextError("");
  }, [refreshKey]);

  const viewAllText = async (source: SourceSummary) => {
    if (!IS_TAURI || !vaultMounted || source.chunk_count === 0) return;
    setSelected(source);
    setTextSourceId(source.source_id);
    setTextPassages([]);
    setTextError("");
    setTextLoading(true);
    const passages: CitationPassage[] = [];
    try {
      for (let ordinal = 0; ordinal < Math.min(source.chunk_count, 64); ordinal += 1) {
        try {
          passages.push(await invoke<CitationPassage>("open_citation", { citationUri: `pinky://source/${source.source_id}/version/${source.version_id}#chunk-${ordinal}` }));
        } catch {
          // Retained chunks may be unavailable while an index repair is in progress.
        }
      }
      if (!passages.length) setTextError("No retained text chunks could be opened.");
      setTextPassages(passages);
    } finally {
      setTextLoading(false);
    }
  };
  const deleteDetectedText = async (source: SourceSummary) => {
    if (!IS_TAURI || !vaultMounted || !window.confirm(`Delete all detected OCR text from “${source.display_name}”? The image, image metadata, and original pixels will be kept. OCR citations will stop resolving.`)) return;
    setTextError("");
    try {
      await invoke<string>("delete_image_ocr", { request: { source_id: source.source_id, version_id: source.version_id } });
      setTextSourceId(null);
      setTextPassages([]);
      setSelected(source);
    } catch (deleteError) {
      setTextError(String(deleteError));
    }
  };
  const deleteDetectedChunk = async (source: SourceSummary, passage: CitationPassage) => {
    if (!IS_TAURI || !vaultMounted || !passage.coordinates?.ocr || !window.confirm("Delete this detected text chunk? The image, image metadata, and other detections will be kept.")) return;
    setTextError("");
    try {
      await invoke<string>("delete_image_ocr_chunk", { request: { source_id: source.source_id, version_id: source.version_id, chunk_id: passage.chunk_id } });
    } catch (deleteError) {
      setTextError(String(deleteError));
    }
  };

  const loadMore = async () => {
    if (!IS_TAURI || !vaultMounted || !nextCursor || loading) return;
    setLoading(true);
    setError("");
    try {
      const response = await invoke<AssetListResponse>("list_assets", { query: { search: debouncedQuery || undefined, kind: kind === "all" ? undefined : kind, state: state === "all" ? undefined : state, sort, cursor: nextCursor, limit: 40 } });
      setItems((current) => [...current, ...response.items]);
      setNextCursor(response.next_cursor);
      setTotal(response.total);
    } catch (loadError) {
      setError(String(loadError));
    } finally {
      setLoading(false);
    }
  };
  const resetFilters = () => { setQuery(""); setKind("all"); setState("all"); setSort("updated"); };
  const selectedCitation = selected && selected.chunk_count > 0 ? `pinky://source/${selected.source_id}/version/${selected.version_id}#chunk-0` : null;
  const imageMetadataPassages = textPassages.filter((passage) => passage.coordinates?.image_metadata || (selected?.mime_type.startsWith("image/") && !passage.coordinates?.ocr && passage.ordinal === 0));
  const detectedTextPassages = textPassages.filter((passage) => passage.coordinates?.ocr);
  const retainedTextPassages = textPassages.filter((passage) => !imageMetadataPassages.includes(passage) && !detectedTextPassages.includes(passage));
  const renderTextChunk = (passage: CitationPassage) => <article className="asset-text-chunk" key={passage.citation_uri}><div><strong>Chunk {passage.ordinal}</strong><span>{passage.coordinates?.ocr ? "Detected text (OCR)" : passage.coordinates?.image_metadata ? "Image metadata" : passage.coordinates?.line_start ? `Lines ${passage.coordinates.line_start}–${passage.coordinates.line_end ?? passage.coordinates.line_start}` : "Retained passage"}</span></div><pre>{passage.passage}</pre><button type="button" onClick={() => onOpenCitation(passage.citation_uri)}>Open citation</button>{passage.coordinates?.ocr && selected && <button type="button" className="danger" onClick={() => void deleteDetectedChunk(selected, passage)}>Delete detection</button>}</article>;

  return <section className="asset-library" aria-label="Asset library">
    <header className="asset-library-header"><div><p className="eyebrow">RETAINED ASSETS</p><h1>Asset library</h1><p>Browse retained sources without loading the whole vault into the sidebar.</p></div><div className="asset-library-actions"><button type="button" className="asset-action" onClick={onAddSource} disabled={!vaultMounted}><Plus size={13} /> Add source</button><button type="button" className="asset-icon-button" onClick={() => setManualRefresh((current) => current + 1)} aria-label="Refresh asset library" title="Refresh asset library"><RefreshCw size={14} /></button></div></header>
    <div className="asset-toolbar"><label className="asset-search"><Search size={14} /><input type="search" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search names and approved paths…" aria-label="Search assets" disabled={!vaultMounted} /></label><label className="asset-filter">Type<select value={kind} onChange={(event) => setKind(event.target.value as typeof kind)} disabled={!vaultMounted}><option value="all">All types</option><option value="local_file">Local files</option><option value="web_page">Web pages</option><option value="search_result">Search results</option><option value="generated">Generated</option></select></label><label className="asset-filter">State<select value={state} onChange={(event) => setState(event.target.value as typeof state)} disabled={!vaultMounted}><option value="all">All states</option><option value="active">Active</option><option value="missing">Missing</option><option value="unsupported">Unsupported</option><option value="blocked">Blocked</option></select></label><label className="asset-filter">Sort<select value={sort} onChange={(event) => setSort(event.target.value as AssetSort)} disabled={!vaultMounted}><option value="updated">Recently updated</option><option value="name">Name</option><option value="size">Size</option><option value="type">Type</option></select></label><div className="asset-view-toggle" role="group" aria-label="Asset layout"><button type="button" className={view === "list" ? "active" : ""} onClick={() => setView("list")} aria-label="List view" aria-pressed={view === "list"}><List size={14} /></button><button type="button" className={view === "grid" ? "active" : ""} onClick={() => setView("grid")} aria-label="Grid view" aria-pressed={view === "grid"}><Grid2X2 size={14} /></button></div></div>
    <div className="asset-library-meta"><span>{loading && !items.length ? "Loading assets…" : `${total.toLocaleString()} asset${total === 1 ? "" : "s"}`}</span>{(query || kind !== "all" || state !== "all" || sort !== "updated") && <button type="button" onClick={resetFilters}>Clear filters</button>}</div>
    {!vaultMounted && <div className="asset-empty"><Database size={25} /><strong>Unlock the vault to browse assets</strong><p>Retained metadata and previews are available only while the encrypted vault is mounted.</p></div>}
    {vaultMounted && error && <div className="asset-error" role="alert"><strong>Could not load assets</strong><p>{error}</p><button type="button" onClick={resetFilters}>Reset filters</button></div>}
    {vaultMounted && !error && !loading && !items.length && <div className="asset-empty"><Database size={25} /><strong>No matching assets</strong><p>{total ? "Try a different filter or search term." : "Add a local source to start building your encrypted library."}</p>{total ? <button type="button" onClick={resetFilters}>Clear filters</button> : <button type="button" onClick={onAddSource}>Add source</button>}</div>}
    {vaultMounted && !!items.length && <div className={`asset-results ${view}`}>{items.map((item) => <article className={`asset-card ${selected?.source_id === item.source_id ? "selected" : ""}`} key={item.source_id}><button type="button" className="asset-main" onClick={() => setSelected(item)}><span className="asset-type">{item.mime_type.startsWith("image/") ? "IMG" : item.kind === "generated" ? "GEN" : item.mime_type.startsWith("text/") ? "TXT" : "FILE"}</span><span className="asset-copy"><strong>{item.display_name}</strong><small>{item.kind.replaceAll("_", " ")} · {item.mime_type} · {formatAssetSize(item.byte_size)}</small><small>{item.state === "active" ? `${item.chunk_count} searchable chunk${item.chunk_count === 1 ? "" : "s"}` : item.state} · updated {new Date(item.updated_at).toLocaleDateString()}</small></span></button><div className="asset-card-actions">{item.chunk_count > 0 && <button type="button" onClick={() => void viewAllText(item)}>View text</button>}{item.state === "active" && item.mime_type.startsWith("image/") && <button type="button" onClick={() => onOpenOcr(item)}>OCR</button>}</div></article>)}</div>}
    {vaultMounted && nextCursor && <button type="button" className="asset-load-more" onClick={() => void loadMore()} disabled={loading}>{loading ? "Loading…" : `Load more · ${Math.max(total - items.length, 0).toLocaleString()} remaining`}</button>}
    {selected && <aside className="asset-detail" aria-label="Selected asset"><header><div><p className="eyebrow">ASSET DETAIL</p><h2>{selected.display_name}</h2></div><button type="button" className="asset-icon-button" aria-label="Close asset detail" onClick={() => { setSelected(null); setTextSourceId(null); }}><X size={14} /></button></header><dl><div><dt>Type</dt><dd>{selected.kind.replaceAll("_", " ")} · {selected.mime_type}</dd></div><div><dt>Size</dt><dd>{formatAssetSize(selected.byte_size)}</dd></div><div><dt>State</dt><dd>{selected.state}</dd></div><div><dt>Updated</dt><dd>{new Date(selected.updated_at).toLocaleString()}</dd></div><div><dt>URI</dt><dd title={selected.canonical_uri}>{selected.canonical_uri}</dd></div></dl><div className="asset-detail-actions">{selectedCitation && <button type="button" className="asset-action" onClick={() => void viewAllText(selected)}>View all text chunks</button>}{selected.state === "active" && selected.mime_type.startsWith("image/") && <button type="button" className="asset-action" onClick={() => onOpenOcr(selected)}>Run / rerun OCR</button>}{selected.mime_type.startsWith("image/") && selected.chunk_count > 1 && <button type="button" className="asset-action danger" onClick={() => void deleteDetectedText(selected)}>Delete detected text</button>}</div></aside>}
    {textSourceId && selected?.source_id === textSourceId && <section className="asset-text-preview" aria-live="polite"><header><div><p className="eyebrow">RETAINED TEXT</p><h3>Metadata and detected text</h3></div><span>{textLoading ? "Loading…" : `${textPassages.length} opened`}</span></header>{textError && <p className="asset-text-error">{textError}</p>}{selected.mime_type.startsWith("image/") ? <><div className="asset-text-section"><h4>Image metadata</h4>{imageMetadataPassages.length ? imageMetadataPassages.map(renderTextChunk) : <p className="asset-text-empty">No image metadata chunk.</p>}</div><div className="asset-text-section"><h4>Detected text (OCR)</h4>{detectedTextPassages.length ? detectedTextPassages.map(renderTextChunk) : <p className="asset-text-empty">No OCR text chunks.</p>}</div></> : <div className="asset-text-section"><h4>Retained text</h4>{retainedTextPassages.map(renderTextChunk)}</div>}</section>}
  </section>;
}

function formatAssetSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function TaskDetail({ task, events }: { task: TaskEvent; events: TaskEvent[] }) {
  const history = [...events].sort((left, right) => left.sequence - right.sequence);
  return <section className="task-detail" id={`task-detail-${task.task_id}`} aria-label={`Details for task ${task.task_id}`}>
    <header><div><p className="eyebrow">TASK DIAGNOSTICS</p><strong>{task.state.replaceAll("_", " ")}</strong></div><code>{task.task_id}</code></header>
    <dl className="task-detail-facts"><div><dt>Tool</dt><dd>{task.tool_name || "not specified"}</dd></div><div><dt>Resource</dt><dd>{task.resource_uri || "not specified"}</dd></div><div><dt>Permission</dt><dd>{task.permission_state}</dd></div><div><dt>Budget</dt><dd>{task.budget_state.replaceAll("_", " ")}</dd></div></dl>
    {task.error && <div className="task-detail-error" role="alert"><strong>{task.error.code}</strong><p>{task.error.message}</p><small>{task.error.recoverable ? "Recoverable failure" : "Non-recoverable failure"}</small></div>}
    <div className="task-detail-heading"><strong>Event timeline</strong><span>{history.length} event{history.length === 1 ? "" : "s"}</span></div>
    <ol className="task-timeline">{history.map((event) => <li className={`task-event ${event.state}`} key={`${event.task_id}-${event.sequence}`}><div className="task-event-heading"><strong>{event.phase.name}</strong><time dateTime={event.timestamp}>{new Date(event.timestamp).toLocaleTimeString()}</time><span>{event.phase.progress == null ? "indeterminate" : `${Math.round(event.phase.progress * 100)}%`}</span></div><p>{event.phase.activity}</p>{event.error && <div className="task-event-error"><strong>{event.error.code}</strong><span>{event.error.message}</span></div>}<details><summary>Event payload · sequence {event.sequence}</summary><pre>{JSON.stringify(event, null, 2)}</pre></details></li>)}</ol>
  </section>;
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
