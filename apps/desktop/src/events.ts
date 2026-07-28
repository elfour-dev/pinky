import type { TaskEvent } from "./types";

export function mergeTaskEvents(current: TaskEvent[], incoming: TaskEvent[]): TaskEvent[] {
  const events = new Map(current.map((event) => [event.sequence, event]));
  for (const event of incoming) events.set(event.sequence, event);
  return [...events.values()]
    .sort((left, right) => left.sequence - right.sequence)
    .slice(-500);
}
