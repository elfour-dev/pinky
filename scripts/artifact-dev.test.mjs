import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const root = mkdtempSync(join(tmpdir(), "pinky-artifact-dev-test-"));
const demoRoot = mkdtempSync(join(tmpdir(), "pinky-artifact-dev-demo-test-"));
const helper = new URL("./artifact-dev.mjs", import.meta.url).pathname;
const run = (...arguments_) => execFileSync(process.execPath, [helper, ...arguments_], { encoding: "utf8" });

try {
  run("init", "--directory", root, "--key-id", "test-key");
  const keyring = join(root, "trusted-artifact-keys.json");
  writeFileSync(keyring, "[]\n");
  assert.match(run("trust-key", "--public-key", join(root, "test-key.public.json"), "--keyring", keyring), /Rebuild Pinky/);
  assert.deepEqual(JSON.parse(readFileSync(keyring, "utf8")), [JSON.parse(readFileSync(join(root, "test-key.public.json"), "utf8"))]);
  assert.throws(
    () => run("trust-key", "--public-key", join(root, "test-key.public.json"), "--keyring", keyring),
    (error) => String(error.stderr).includes("already contains key_id"),
  );
  const artifact = join(root, "qdrant-example");
  const unsigned = join(root, "unsigned.json");
  const signed = join(root, "signed.json");
  writeFileSync(artifact, "development artifact bytes");
  run("create-manifest", "--artifact", artifact, "--artifact-id", "test-qdrant", "--kind", "executable", "--capability", "vector_database", "--version", "1.2.3", "--url", "https://example.invalid/qdrant", "--license-url", "https://example.invalid/license", "--runtime-version", "linux-x86_64", "--context-length", "none", "--minimum-ram-bytes", "1073741824", "--out", unsigned);
  assert.equal(JSON.parse(readFileSync(unsigned, "utf8")).byte_size, 26);
  run("sign", "--key", join(root, "test-key.private.pem"), "--key-id", "test-key", "--manifest", unsigned, "--out", signed);
  assert.match(run("verify", "--public-key", join(root, "test-key.public.json"), "--manifest", signed), /valid/);

  const tampered = JSON.parse(readFileSync(signed, "utf8"));
  tampered.manifest.version = "9.9.9";
  writeFileSync(signed, `${JSON.stringify(tampered)}\n`);
  assert.throws(
    () => run("verify", "--public-key", join(root, "test-key.public.json"), "--manifest", signed),
    (error) => String(error.stderr).includes("signature is not valid"),
  );
  run("demo", "--directory", demoRoot, "--key-id", "demo-key");
  assert.match(run("verify", "--public-key", join(demoRoot, "demo-key.public.json"), "--manifest", join(demoRoot, "signed-manifest.json")), /valid/);
  process.stdout.write("artifact development helper tests passed\n");
} finally {
  rmSync(root, { recursive: true, force: true });
  rmSync(demoRoot, { recursive: true, force: true });
}
