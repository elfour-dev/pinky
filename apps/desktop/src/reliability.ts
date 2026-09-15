export function isReconnectableError(error: string) {
  const normalized = error.toLowerCase();
  return normalized.includes("unavailable") || normalized.includes("timed out") || normalized.includes("request failed") || normalized.includes("http 5");
}
