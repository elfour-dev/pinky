import { existsSync, lstatSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const scriptName = path.basename(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);

function valuesFor(name) {
  const values = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === name) {
      const value = args[index + 1];
      if (!value || value.startsWith("--")) throw new Error(`${name} requires a value`);
      values.push(value);
      index += 1;
    }
  }
  return values;
}

function required(name) {
  const value = valuesFor(name)[0];
  if (!value) throw new Error(`missing ${name}`);
  return path.resolve(value);
}

function usage() {
  console.error(`Usage: node ${scriptName} --root PROFILE --vault MOUNT [--marker TEXT ...]`);
  console.error("Scans regular files outside the verified vault mount for retained-data markers.");
}

if (args.includes("--help")) {
  usage();
  process.exit(0);
}

const root = required("--root");
const vault = required("--vault");
if (!existsSync(root) || !lstatSync(root).isDirectory()) throw new Error(`root is not a directory: ${root}`);
if (!existsSync(vault) || !lstatSync(vault).isDirectory()) throw new Error(`vault is not a directory: ${vault}`);

const markers = valuesFor("--marker");
const needles = markers.length > 0
  ? markers
  : ["pinky://source/", "BEGIN-pinky-evidence-", '"summary_citations"', '"unresolved_gaps"'];
const maximumBytes = 16 * 1024 * 1024;
const findings = [];
const skipped = [];

function isInside(candidate, parent) {
  const relative = path.relative(parent, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function scan(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const candidate = path.join(directory, entry.name);
    if (isInside(candidate, vault)) {
      skipped.push(candidate);
      continue;
    }
    if (entry.isSymbolicLink()) {
      skipped.push(candidate);
      continue;
    }
    if (entry.isDirectory()) {
      scan(candidate);
      continue;
    }
    if (!entry.isFile()) {
      skipped.push(candidate);
      continue;
    }
    const size = lstatSync(candidate).size;
    if (size > maximumBytes) {
      skipped.push(candidate);
      continue;
    }
    const content = readFileSync(candidate);
    for (const needle of needles) {
      const offset = content.indexOf(Buffer.from(needle));
      if (offset !== -1) findings.push({ file: candidate, marker: needle, byte_offset: offset });
    }
  }
}

scan(root);
const report = {
  schema_version: 1,
  root,
  excluded_vault: vault,
  markers: needles,
  findings,
  skipped_files: skipped.length,
};
console.log(JSON.stringify(report, null, 2));
if (findings.length > 0) process.exitCode = 1;
