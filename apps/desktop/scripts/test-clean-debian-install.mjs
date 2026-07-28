import { readdirSync } from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repository = path.resolve(desktop, "../..");
const debDirectory = path.join(repository, "target/release/bundle/deb");
const packages = readdirSync(debDirectory).filter((entry) => entry.endsWith(".deb"));
if (packages.length !== 1) throw new Error(`Expected one Debian package, found ${packages.length}`);

const deb = path.join(debDirectory, packages[0]);
const acceptance = path.join(desktop, "scripts/accept-clean-debian-install.sh");
const image = "debian@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd";
const result = spawnSync("docker", [
  "run", "--rm",
  "--cpus", "2",
  "--memory", "4g",
  "--pids-limit", "256",
  "--shm-size", "512m",
  "--mount", `type=bind,src=${deb},dst=/tmp/Pinky.deb,readonly`,
  "--mount", `type=bind,src=${acceptance},dst=/tmp/accept.sh,readonly`,
  image,
  "/bin/sh", "/tmp/accept.sh", "/tmp/Pinky.deb",
], { stdio: "inherit" });

if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);
