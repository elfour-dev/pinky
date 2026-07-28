import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import net from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const directory = path.dirname(fileURLToPath(import.meta.url));
const repository = path.resolve(directory, "../../..");
const application = path.join(repository, "target/debug/pinky-desktop");
const profile = mkdtempSync(path.join(tmpdir(), "pinky-e2e-"));
const driverEnvironment = {
  ...process.env,
  PINKY_E2E_PROFILE: profile,
  XDG_CONFIG_HOME: path.join(profile, "config"),
  XDG_DATA_HOME: path.join(profile, "data"),
  XDG_CACHE_HOME: path.join(profile, "cache"),
};
const driver = spawn(process.env.TAURI_DRIVER || "tauri-driver", [], {
  env: driverEnvironment,
  stdio: ["ignore", "inherit", "inherit"],
});

let sessionId;

async function request(method, endpoint, body) {
  const response = await fetch(`http://127.0.0.1:4444${endpoint}`, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const payload = await response.json();
  if (!response.ok || payload.value?.error) {
    throw new Error(`WebDriver ${method} ${endpoint} failed: ${JSON.stringify(payload)}`);
  }
  return payload.value;
}

async function execute(script, args = []) {
  return request("POST", `/session/${sessionId}/execute/sync`, { script, args });
}

async function waitUntil(script, message, timeoutMilliseconds = 5_000) {
  const deadline = Date.now() + timeoutMilliseconds;
  while (Date.now() < deadline) {
    if (await execute(script)) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(message);
}

async function clickButton(label) {
  const clicked = await execute(`
    const button = [...document.querySelectorAll('button')]
      .find((candidate) => candidate.textContent.trim() === arguments[0]);
    if (!button) return false;
    button.click();
    return true;
  `, [label]);
  assert.equal(clicked, true, `button ${label} was not found`);
}

function waitForPort(port) {
  return new Promise((resolve, reject) => {
    const deadline = Date.now() + 10_000;
    const attempt = () => {
      const socket = net.createConnection({ host: "127.0.0.1", port });
      socket.once("connect", () => {
        socket.destroy();
        resolve();
      });
      socket.once("error", () => {
        socket.destroy();
        if (Date.now() >= deadline) reject(new Error(`tauri-driver did not listen on port ${port}`));
        else setTimeout(attempt, 50);
      });
    };
    attempt();
  });
}

try {
  await waitForPort(4444);
  const session = await request("POST", "/session", {
    capabilities: {
      alwaysMatch: { "tauri:options": { application } },
    },
  });
  sessionId = session.sessionId;
  await waitUntil("return Boolean(document.querySelector('main.app-shell'))", "Pinky shell did not load");

  assert.equal(await request("GET", `/session/${sessionId}/title`), "Pinky");
  assert.equal(await execute("return document.querySelectorAll('aside.left-panel, section.centre-panel, aside.right-panel').length"), 3);
  assert.match(await execute("return document.querySelector('.vault-card').textContent"), /Vault locked/);
  assert.equal(await execute("return document.querySelector('button.send').disabled"), true);
  assert.equal(await execute("return document.querySelector('button.new-chat').disabled"), true);
  console.log("PASS native three-region shell");

  await clickButton("Start setup");
  await waitUntil("return Boolean(document.querySelector('.setup-modal'))", "setup modal did not open");
  const paths = await execute("return [...document.querySelectorAll('.setup-modal input')].slice(0, 2).map((input) => input.value)");
  assert.ok(paths[0].startsWith(profile));
  assert.ok(paths[1].startsWith(profile));
  assert.notEqual(paths[0], paths[1]);
  assert.equal(await execute("document.querySelector(\"button[aria-label='Close setup']\").click(); return true"), true);
  await waitUntil("return !document.querySelector('.setup-modal')", "setup modal did not close");
  console.log("PASS native default-vault-path command");

  await clickButton("Run system check");
  await waitUntil("return Boolean(document.querySelector('article.task-card button'))", "task controls did not appear");
  await clickButton("Stop");
  await waitUntil(
    "return document.querySelector('article.task-card')?.classList.contains('cancelled')",
    "system-check task did not reach cancelled state",
  );
  assert.match(await execute("return document.querySelector('article.task-card').textContent"), /Cancelled/);
  console.log("PASS native task events and cancellation command");
} finally {
  if (sessionId) {
    await request("DELETE", `/session/${sessionId}`).catch(() => undefined);
  }
  driver.kill("SIGTERM");
  rmSync(profile, { recursive: true, force: true });
}
