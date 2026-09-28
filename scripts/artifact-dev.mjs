#!/usr/bin/env node
/**
 * Development helper for Pinky's signed-artifact contract.
 *
 * This intentionally creates a development key outside the checkout.  It is
 * useful for exercising the verifier and release procedure, but its key must
 * never be presented as a production Pinky release key.
 */
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, sign, verify } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, statSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";

const CONTEXT = Buffer.from("pinky-artifact-manifest-v1\0", "utf8");
const REQUIRED_FIELDS = [
  "schema_version", "artifact_id", "kind", "capability", "version", "url",
  "sha256", "byte_size", "license_url", "runtime_version", "context_length",
  "minimum_ram_bytes",
];
const USAGE = `Usage:
  scripts/artifact-dev.mjs init [--directory ABSOLUTE_DIRECTORY] [--key-id KEY_ID]
  scripts/artifact-dev.mjs demo [--directory ABSOLUTE_DIRECTORY] [--key-id KEY_ID]
  scripts/artifact-dev.mjs create-manifest --artifact FILE --artifact-id ID --kind model|executable --capability NAME --version VERSION --url HTTPS_URL --license-url HTTPS_URL --runtime-version NAME --context-length NUMBER|none --minimum-ram-bytes NUMBER --out unsigned.json
  scripts/artifact-dev.mjs sign --key PRIVATE_KEY.pem --key-id KEY_ID --manifest unsigned.json --out signed.json
  scripts/artifact-dev.mjs verify --public-key public-key.json --manifest signed.json
  scripts/artifact-dev.mjs trust-key --public-key public-key.json --keyring trusted-artifact-keys.json

Run \`init\` once to create a development-only keypair outside this checkout.
The public-key JSON file can be given to \`verify\`; keep the PEM private key secret.
\`trust-key\` changes an explicit keyring path after checking the public key; it never
accepts or writes a private key.
`;

function fail(message) {
  process.stderr.write(`artifact-dev: ${message}\n`);
  process.exit(1);
}

function parseOptions(arguments_) {
  const options = new Map();
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    if (!argument.startsWith("--")) fail(`unexpected argument: ${argument}`);
    const value = arguments_[index + 1];
    if (!value || value.startsWith("--")) fail(`missing value for ${argument}`);
    options.set(argument.slice(2), value);
    index += 1;
  }
  return options;
}

function option(options, name) {
  const value = options.get(name);
  if (!value) fail(`--${name} is required`);
  return value;
}

function positiveInteger(value, label) {
  if (!/^[1-9][0-9]*$/.test(value)) fail(`${label} must be a positive integer`);
  const number = Number(value);
  if (!Number.isSafeInteger(number)) fail(`${label} is too large for this development helper`);
  return number;
}

function defaultDirectory() {
  const state = process.env.XDG_STATE_HOME || (process.env.HOME && join(process.env.HOME, ".local", "state"));
  if (!state) fail("set XDG_STATE_HOME or HOME so the development key can live outside the checkout");
  return join(state, "pinky", "artifact-dev");
}

function requireAbsolute(path, label) {
  if (!isAbsolute(path)) fail(`${label} must be an absolute path`);
  return resolve(path);
}

function loadJson(path, label) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    fail(`could not read ${label} ${path}: ${error.message}`);
  }
}

function orderedManifest(manifest) {
  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
    fail("the unsigned manifest must be a JSON object");
  }
  const supplied = Object.keys(manifest).sort();
  const expected = [...REQUIRED_FIELDS].sort();
  if (supplied.length !== expected.length || supplied.some((field, index) => field !== expected[index])) {
    fail(`the unsigned manifest must contain exactly: ${REQUIRED_FIELDS.join(", ")}`);
  }
  if (manifest.schema_version !== 1) fail("schema_version must be 1");
  if (!["model", "executable"].includes(manifest.kind)) fail("kind must be model or executable");
  for (const field of ["artifact_id", "capability", "version", "url", "sha256", "license_url", "runtime_version"]) {
    if (typeof manifest[field] !== "string" || !manifest[field] || /\s/.test(manifest[field])) {
      fail(`${field} must be a non-empty string without whitespace`);
    }
  }
  for (const field of ["url", "license_url"]) {
    if (!manifest[field].startsWith("https://")) fail(`${field} must use HTTPS`);
  }
  if (!/^[0-9a-f]{64}$/.test(manifest.sha256)) fail("sha256 must be 64 lowercase hexadecimal characters");
  if (!Number.isSafeInteger(manifest.byte_size) || manifest.byte_size <= 0) fail("byte_size must be a positive safe integer");
  if (manifest.context_length !== null && (!Number.isSafeInteger(manifest.context_length) || manifest.context_length <= 0)) {
    fail("context_length must be null or a positive safe integer");
  }
  if (!Number.isSafeInteger(manifest.minimum_ram_bytes) || manifest.minimum_ram_bytes <= 0) {
    fail("minimum_ram_bytes must be a positive safe integer");
  }
  // Keep this order aligned with Rust's ArtifactManifestV1 serialization.
  return Object.fromEntries(REQUIRED_FIELDS.map((field) => [field, manifest[field]]));
}

function canonicalPayload(manifest) {
  return Buffer.concat([CONTEXT, Buffer.from(JSON.stringify(orderedManifest(manifest)), "utf8")]);
}

function init(options) {
  const directory = requireAbsolute(options.get("directory") || defaultDirectory(), "--directory");
  const keyId = options.get("key-id") || "development-key-1";
  const { privatePath, publicPath } = createDevelopmentKey(directory, keyId);
  process.stdout.write(`Created a DEVELOPMENT-ONLY signing key.\n\nPrivate key (keep secret; never commit):\n  ${privatePath}\n\nPublic key (safe to distribute with the app):\n  ${publicPath}\n\nNext: create an unsigned manifest, then run the sign command shown by:\n  ${process.argv[1]} sign --help\n`);
}

function createDevelopmentKey(directory, keyId) {
  if (/\s/.test(keyId)) fail("key ID must not contain whitespace");
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  chmodSync(directory, 0o700);
  const privatePath = join(directory, `${keyId}.private.pem`);
  const publicPath = join(directory, `${keyId}.public.json`);
  if (existsSync(privatePath) || existsSync(publicPath)) fail(`refusing to overwrite an existing key at ${directory}`);
  const pair = generateKeyPairSync("ed25519");
  const publicJwk = pair.publicKey.export({ format: "jwk" });
  writeFileSync(privatePath, pair.privateKey.export({ format: "pem", type: "pkcs8" }), { mode: 0o600 });
  chmodSync(privatePath, 0o600);
  writeFileSync(publicPath, `${JSON.stringify({ key_id: keyId, public_key_base64: Buffer.from(publicJwk.x, "base64url").toString("base64") }, null, 2)}\n`, { mode: 0o644 });
  return { privatePath, publicPath };
}

function demo(options) {
  const directory = requireAbsolute(options.get("directory") || join(defaultDirectory(), `demo-${Date.now()}`), "--directory");
  const keyId = options.get("key-id") || "development-key-1";
  const { privatePath, publicPath } = createDevelopmentKey(directory, keyId);
  const artifactPath = join(directory, "development-example.bin");
  const unsignedPath = join(directory, "unsigned-manifest.json");
  const signedPath = join(directory, "signed-manifest.json");
  writeFileSync(artifactPath, "Pinky development-only artifact. This file is safe to delete.\n", { mode: 0o600 });
  const metadata = statSync(artifactPath);
  const manifest = orderedManifest({
    schema_version: 1,
    artifact_id: "pinky-development-example",
    kind: "executable",
    capability: "vector_database",
    version: "0.0.1",
    url: "https://example.invalid/pinky-development-example",
    sha256: createHash("sha256").update(readFileSync(artifactPath)).digest("hex"),
    byte_size: metadata.size,
    license_url: "https://example.invalid/license",
    runtime_version: "linux-x86_64",
    context_length: null,
    minimum_ram_bytes: 1048576,
  });
  writeFileSync(unsignedPath, `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o600 });
  const signature = sign(null, canonicalPayload(manifest), createPrivateKey(readFileSync(privatePath))).toString("base64");
  writeFileSync(signedPath, `${JSON.stringify({ key_id: keyId, manifest, signature }, null, 2)}\n`, { mode: 0o600 });
  const key = loadJson(publicPath, "development public key");
  const publicKey = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: Buffer.from(key.public_key_base64, "base64").toString("base64url") }, format: "jwk" });
  if (!verify(null, canonicalPayload(manifest), publicKey, Buffer.from(signature, "base64"))) fail("internal development-demo verification failed");
  process.stdout.write(`Development signing demo complete. Nothing was downloaded or installed.\n\nAll temporary development materials are in:\n  ${directory}\n\nCreated:\n  ${artifactPath}\n  ${unsignedPath}\n  ${signedPath}\n  ${publicPath}\n  ${privatePath}\n\nYou may inspect these files, rerun verify, or delete the directory when finished.\n`);
}

function createManifest(options) {
  const artifactPath = requireAbsolute(option(options, "artifact"), "--artifact");
  const outputPath = requireAbsolute(option(options, "out"), "--out");
  if (existsSync(outputPath)) fail(`refusing to overwrite ${outputPath}`);
  let metadata;
  try {
    metadata = statSync(artifactPath, { throwIfNoEntry: true });
  } catch (error) {
    fail(`could not inspect artifact ${artifactPath}: ${error.message}`);
  }
  if (!metadata.isFile()) fail("--artifact must name a regular file");
  if (!Number.isSafeInteger(metadata.size) || metadata.size <= 0) fail("artifact must have a positive size supported by this development helper");
  const contextLength = option(options, "context-length");
  const manifest = orderedManifest({
    schema_version: 1,
    artifact_id: option(options, "artifact-id"),
    kind: option(options, "kind"),
    capability: option(options, "capability"),
    version: option(options, "version"),
    url: option(options, "url"),
    sha256: createHash("sha256").update(readFileSync(artifactPath)).digest("hex"),
    byte_size: metadata.size,
    license_url: option(options, "license-url"),
    runtime_version: option(options, "runtime-version"),
    context_length: contextLength === "none" ? null : positiveInteger(contextLength, "--context-length"),
    minimum_ram_bytes: positiveInteger(option(options, "minimum-ram-bytes"), "--minimum-ram-bytes"),
  });
  mkdirSync(dirname(outputPath), { recursive: true, mode: 0o700 });
  writeFileSync(outputPath, `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o600 });
  process.stdout.write(`Unsigned manifest written to:\n  ${outputPath}\n\nReview every field before signing it.\n`);
}

function signManifest(options) {
  const keyPath = requireAbsolute(option(options, "key"), "--key");
  const manifestPath = requireAbsolute(option(options, "manifest"), "--manifest");
  const outputPath = requireAbsolute(option(options, "out"), "--out");
  const keyId = option(options, "key-id");
  if (/\s/.test(keyId)) fail("key ID must not contain whitespace");
  if (existsSync(outputPath)) fail(`refusing to overwrite ${outputPath}`);
  const manifest = orderedManifest(loadJson(manifestPath, "unsigned manifest"));
  const privateKey = createPrivateKey(readFileSync(keyPath));
  const signature = sign(null, canonicalPayload(manifest), privateKey).toString("base64");
  mkdirSync(dirname(outputPath), { recursive: true, mode: 0o700 });
  writeFileSync(outputPath, `${JSON.stringify({ key_id: keyId, manifest, signature }, null, 2)}\n`, { mode: 0o600 });
  process.stdout.write(`Signed manifest written to:\n  ${outputPath}\n`);
}

function verifyManifest(options) {
  const keyPath = requireAbsolute(option(options, "public-key"), "--public-key");
  const manifestPath = requireAbsolute(option(options, "manifest"), "--manifest");
  const key = loadJson(keyPath, "public key");
  const signed = loadJson(manifestPath, "signed manifest");
  if (!key || typeof key.key_id !== "string" || typeof key.public_key_base64 !== "string") fail("public key must contain key_id and public_key_base64");
  if (!signed || signed.key_id !== key.key_id || typeof signed.signature !== "string") fail("signed manifest key_id does not match the public key");
  const publicKey = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: Buffer.from(key.public_key_base64, "base64").toString("base64url") }, format: "jwk" });
  if (!verify(null, canonicalPayload(signed.manifest), publicKey, Buffer.from(signed.signature, "base64"))) fail("signature is not valid");
  process.stdout.write(`Manifest is valid for development key ${key.key_id}.\n`);
}

function trustedPublicKey(path) {
  const key = loadJson(path, "public key");
  if (!key || typeof key.key_id !== "string" || !key.key_id || /\s/.test(key.key_id)) {
    fail("public key must contain a non-empty key_id without whitespace");
  }
  if (typeof key.public_key_base64 !== "string" || !/^[A-Za-z0-9+/]{43}=$/.test(key.public_key_base64)) {
    fail("public key must contain a canonical base64 Ed25519 public key");
  }
  const bytes = Buffer.from(key.public_key_base64, "base64");
  if (bytes.length !== 32 || bytes.toString("base64") !== key.public_key_base64) {
    fail("public key must contain exactly 32 canonical base64 bytes");
  }
  return { key_id: key.key_id, public_key_base64: key.public_key_base64 };
}

function trustKey(options) {
  const publicKeyPath = requireAbsolute(option(options, "public-key"), "--public-key");
  const keyringPath = requireAbsolute(option(options, "keyring"), "--keyring");
  const key = trustedPublicKey(publicKeyPath);
  const keyring = loadJson(keyringPath, "trusted artifact keyring");
  if (!Array.isArray(keyring)) fail("trusted artifact keyring must be a JSON array");
  if (keyring.some((entry) => !entry || typeof entry !== "object" || Array.isArray(entry))) {
    fail("trusted artifact keyring entries must be JSON objects");
  }
  if (keyring.some((entry) => entry.key_id === key.key_id)) {
    fail(`trusted artifact keyring already contains key_id ${key.key_id}`);
  }
  const updated = [...keyring, key];
  const temporaryPath = `${keyringPath}.pinky-artifact-key-${process.pid}.tmp`;
  if (existsSync(temporaryPath)) fail(`refusing to overwrite temporary keyring path ${temporaryPath}`);
  try {
    writeFileSync(temporaryPath, `${JSON.stringify(updated, null, 2)}\n`, { mode: 0o644 });
    renameSync(temporaryPath, keyringPath);
  } catch (error) {
    fail(`could not update trusted artifact keyring ${keyringPath}: ${error.message}`);
  }
  process.stdout.write(`Trusted public key ${key.key_id} added to:\n  ${keyringPath}\n\nRebuild Pinky before using manifests signed by this key.\n`);
}

const [command, ...arguments_] = process.argv.slice(2);
if (!command || command === "--help" || command === "-h") {
  process.stdout.write(USAGE);
} else if (arguments_.includes("--help") || arguments_.includes("-h")) {
  process.stdout.write(USAGE);
} else {
  const options = parseOptions(arguments_);
  if (command === "init") init(options);
  else if (command === "demo") demo(options);
  else if (command === "create-manifest") createManifest(options);
  else if (command === "sign") signManifest(options);
  else if (command === "verify") verifyManifest(options);
  else if (command === "trust-key") trustKey(options);
  else fail(`unknown command: ${command}`);
}
