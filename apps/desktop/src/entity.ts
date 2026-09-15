import type { TaskState } from "./types";
export type EntityState = "idle" | "listening" | "thinking" | "researching" | "creating" | "waiting" | "cancelling" | "error" | "completed";
export function deriveEntityState(tasks: Array<{ state: TaskState; phase: { name: string }; sequence?: number }>, listening = false): EntityState {
  // The task panel intentionally retains terminal history. Only the newest
  // active task should drive ALMA, otherwise an old failed request would keep
  // a later conversation permanently in the error expression.
  const ordered = [...tasks].sort((left, right) => (right.sequence ?? 0) - (left.sequence ?? 0));
  const active = ordered.find((task) => ["cancelling", "waiting_for_user", "queued", "running"].includes(task.state));
  if (active?.state === "cancelling") return "cancelling";
  if (active?.state === "waiting_for_user") return "waiting";
  const running = active && (active.state === "queued" || active.state === "running") ? active : undefined;
  if (running) {
    if (/research|fetch|search/i.test(running.phase.name)) return "researching";
    if (/creat|generat|build|image/i.test(running.phase.name)) return "creating";
    return "thinking";
  }
  if (ordered[0]?.state === "failed" || ordered[0]?.state === "failed_interrupted") return "error";
  if (ordered[0]?.state === "completed") return "completed";
  return listening ? "listening" : "idle";
}
