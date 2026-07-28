import { createHash } from "node:crypto";
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repository = path.resolve(desktop, "../..");
const bundleRoot = path.join(repository, "target/release/bundle");

function requireSingle(directory, suffix) {
  const matches = readdirSync(directory)
    .filter((entry) => entry.endsWith(suffix))
    .map((entry) => path.join(directory, entry));
  if (matches.length !== 1) throw new Error(`Expected one ${suffix} in ${directory}, found ${matches.length}`);
  return matches[0];
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", ...options });
  if (result.status !== 0) throw new Error(`${program} ${args.join(" ")} failed:\n${result.stderr || result.stdout}`);
  return result.stdout.trim();
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

const deb = requireSingle(path.join(bundleRoot, "deb"), ".deb");
const appImage = requireSingle(path.join(bundleRoot, "appimage"), ".AppImage");
const architecture = run("dpkg-deb", ["-f", deb, "Architecture"]);
if (architecture !== "amd64") throw new Error(`Unexpected Debian architecture: ${architecture}`);
const debContents = run("dpkg-deb", ["--contents", deb]);
for (const required of ["usr/bin/pinky-desktop", "applications/Pinky.desktop", "icons/"]) {
  if (!debContents.includes(required)) throw new Error(`Debian bundle is missing ${required}`);
}

const appImageMode = statSync(appImage).mode;
if ((appImageMode & 0o111) === 0) throw new Error("AppImage is not executable");
const extraction = mkdtempSync(path.join(tmpdir(), "pinky-appimage-"));
try {
  run(appImage, ["--appimage-extract"], { cwd: extraction });
  const root = path.join(extraction, "squashfs-root");
  for (const required of ["AppRun", "Pinky.desktop", "usr/bin/pinky-desktop"]) {
    statSync(path.join(root, required));
  }
} finally {
  rmSync(extraction, { recursive: true, force: true });
}

console.log(JSON.stringify({
  schema_version: 1,
  architecture,
  artifacts: [deb, appImage].map((file) => ({
    file: path.relative(repository, file),
    bytes: statSync(file).size,
    sha256: sha256(file),
  })),
}, null, 2));
