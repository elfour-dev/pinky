export type TaskState = "queued" | "running" | "waiting_for_user" | "cancelling" | "cancelled" | "completed" | "failed" | "failed_interrupted";
export interface TaskEvent {
  schema_major: number; schema_minor: number; sequence: number; task_id: string; parent_id: string | null; timestamp: string; state: TaskState;
  phase: { name: string; progress: number | null; activity: string }; tool_name: string | null; resource_uri: string | null;
  permission_state: string; budget_state: string; cancellable: boolean;
  error: { code: string; message: string; recoverable: boolean } | null;
}
export interface RuntimeStatus { vault_mounted: boolean; prerequisites: Record<string, boolean>; }
