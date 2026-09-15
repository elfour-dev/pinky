export type TaskState = "queued" | "running" | "waiting_for_user" | "cancelling" | "cancelled" | "completed" | "failed" | "failed_interrupted";
export interface TaskEvent {
  schema_major: number; schema_minor: number; sequence: number; task_id: string; parent_id: string | null; timestamp: string; state: TaskState;
  phase: { name: string; progress: number | null; activity: string }; tool_name: string | null; resource_uri: string | null;
  permission_state: string; budget_state: string; cancellable: boolean;
  error: { code: string; message: string; recoverable: boolean } | null;
}
export interface RuntimeStatus {
  vault_mounted: boolean;
  setup_in_progress: boolean;
  vault_registered: boolean;
  vault_id: string | null;
  unlock_error: string | null;
  task_journal_error: string | null;
  watcher_error: string | null;
  model_attach_in_progress: boolean;
  model_connected: boolean;
  model_provider: string | null;
  model_name: string | null;
  model_context_size: number | null;
  model_error: string | null;
  hybrid_configured: boolean;
  hybrid_model: string | null;
  prerequisites: Record<string, boolean>;
}
export interface VaultPaths { cipher_dir: string; mount_dir: string; }
export interface SetupVaultResponse { vault_id: string; recovery_path: string; }
export interface AttachLlamaResponse { provider: string; model_name: string; context_size: number; total_slots: number; }
export interface HybridConfiguration { qdrant_executable: string; embedding_endpoint: string; embedding_model: string; }
export interface SourceSummary {
  source_id: string;
  version_id: string;
  display_name: string;
  canonical_uri: string;
  mime_type: string;
  byte_size: number;
  chunk_count: number;
  state: string;
  updated_at: string;
}
export interface SearchHit {
  score: number;
  citation_uri: string;
  source_id: string;
  version_id: string;
  chunk_id: string;
  ordinal: number;
  display_name: string;
  heading: string | null;
  passage: string;
  coordinates: { line_start?: number; line_end?: number } | null;
  retrieved_at: string;
}
export interface CitationPassage extends Omit<SearchHit, "score"> {
  canonical_uri: string;
  mime_type: string;
}
export type ClaimSupport = "direct" | "inference" | "disputed";
export interface AnswerClaim {
  statement: string;
  support: ClaimSupport;
  citations: string[];
}
export interface AnswerEnvelope {
  schema_version: number;
  summary: string;
  summary_citations: string[];
  claims: AnswerClaim[];
  warnings: string[];
  unresolved_gaps: string[];
}
export interface ConversationSummary {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
}
export interface ConversationMessage {
  id: string;
  conversation_id: string;
  ordinal: number;
  role: "user" | "assistant";
  content: string;
  model: string | null;
  citations: string[];
  task_id: string | null;
  replaces_message_id: string | null;
  created_at: string;
}
export interface ConversationDetail {
  summary: ConversationSummary;
  messages: ConversationMessage[];
}
export interface AskQuestionResponse {
  conversation_id: string;
  answer: AnswerEnvelope;
}

export function canAsk(status: RuntimeStatus, question: string, sourceCount: number): boolean {
  return status.vault_mounted && status.model_connected && sourceCount > 0 && question.trim().length > 0;
}
