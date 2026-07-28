import type { TaskState } from "./types";
export type EntityState = "idle" | "listening" | "thinking" | "researching" | "creating" | "waiting" | "cancelling" | "error" | "completed";
export function deriveEntityState(tasks: Array<{ state: TaskState; phase: { name: string } }>, listening = false): EntityState {
  if (tasks.some((task) => task.state === "cancelling")) return "cancelling";
  if (tasks.some((task) => task.state === "failed" || task.state === "failed_interrupted")) return "error";
  if (tasks.some((task) => task.state === "waiting_for_user")) return "waiting";
  const running = tasks.find((task) => task.state === "running");
  if (running) {
    if (/research|fetch|search/i.test(running.phase.name)) return "researching";
    if (/creat|generat|build|image/i.test(running.phase.name)) return "creating";
    return "thinking";
  }
  if (tasks.some((task) => task.state === "completed")) return "completed";
  return listening ? "listening" : "idle";
}
