#!/usr/bin/env node
// Mirror a published GitHub release to CNB (https://cnb.cool).
//
//   CNB_TOKEN=... CNB_REPOSITORY=yisier1/rustgrid \
//   node .github/scripts/sync-cnb-release.mjs --tag v0.1.19 \
//     --name "RustGrid 0.1.19" --body-file release-body.md --assets-dir dist
//
// The release's tag must already exist in the CNB repository (the workflow mirrors the Git refs
// first). `latest.json` — the updater manifest — is uploaded **last**, so the release's `latest`
// alias never resolves to a manifest that lists archives the release does not have yet.
//
// The create/update -> asset-upload-url -> PUT -> confirm flow is the one t8y2/dbx uses in its
// Apache-2.0 `.github/scripts/sync-cnb-release.mjs`.

import { createReadStream, existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { basename, join } from "node:path";

const API_BASE = (process.env.CNB_API_BASE || "https://api.cnb.cool").replace(/\/+$/, "");
const REPOSITORY = (process.env.CNB_REPOSITORY || "yisier1/rustgrid").replace(/^\/+|\/+$/g, "");
const TOKEN = (process.env.CNB_TOKEN || "").trim();
const MANIFEST = "latest.json";
const CONCURRENCY = Math.max(1, Number.parseInt(process.env.CNB_UPLOAD_CONCURRENCY || "3", 10) || 3);
const MAX_ATTEMPTS = 4;

function parseArgs(argv) {
  const args = { tag: "", name: "", bodyFile: "", assetsDir: "", overwrite: true, prerelease: false };
  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index];
    if (arg === "--tag") args.tag = argv[++index];
    else if (arg === "--name") args.name = argv[++index];
    else if (arg === "--body-file") args.bodyFile = argv[++index];
    else if (arg === "--assets-dir") args.assetsDir = argv[++index];
    else if (arg === "--no-overwrite") args.overwrite = false;
    else if (arg === "--prerelease") args.prerelease = true;
    else throw new Error(`Unknown argument: ${arg}`);
  }
  if (!args.tag) throw new Error("Missing --tag");
  if (!args.assetsDir) throw new Error("Missing --assets-dir");
  if (!TOKEN) throw new Error("CNB_TOKEN is required");
  if (!existsSync(args.assetsDir)) throw new Error(`No such directory: ${args.assetsDir}`);
  return args;
}

function headers(json) {
  return {
    Accept: "application/vnd.cnb.api+json",
    Authorization: `Bearer ${TOKEN}`,
    ...(json ? { "Content-Type": "application/json" } : {}),
  };
}

async function request(method, path, body = null, allow404 = false) {
  const response = await fetch(`${API_BASE}${path}`, {
    method,
    headers: headers(body !== null),
    body: body === null ? undefined : JSON.stringify(body),
  });
  if (allow404 && response.status === 404) return null;
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`CNB API ${method} ${path} -> ${response.status}: ${text.trim()}`);
  }
  // CNB acknowledges some metadata updates with an empty body.
  return text.trim() ? JSON.parse(text) : null;
}

async function findRelease(tag) {
  return request("GET", `/${REPOSITORY}/-/releases/tags/${encodeURIComponent(tag)}`, null, true);
}

async function ensureRelease({ tag, name, body, prerelease }) {
  const existing = await findRelease(tag);
  const payload = {
    tag_name: tag,
    name: name || tag,
    body,
    prerelease,
    make_latest: "true",
  };
  if (!existing) {
    console.log(`Creating CNB release ${tag}.`);
    await request("POST", `/${REPOSITORY}/-/releases`, payload);
  } else {
    console.log(`Updating CNB release ${tag} (${existing.id}).`);
    await request("PATCH", `/${REPOSITORY}/-/releases/${encodeURIComponent(existing.id)}`, {
      name: payload.name,
      body: payload.body,
      prerelease: payload.prerelease,
      make_latest: payload.make_latest,
    });
  }
  const release = await findRelease(tag);
  if (!release?.id) throw new Error(`CNB release ${tag} is missing after create/update`);
  return release;
}

async function uploadAsset(releaseId, filePath, overwrite) {
  const name = basename(filePath);
  const size = statSync(filePath).size;
  const target = await request(
    "POST",
    `/${REPOSITORY}/-/releases/${encodeURIComponent(releaseId)}/asset-upload-url`,
    { asset_name: name, size, overwrite },
  );
  if (!target?.upload_url) throw new Error(`No upload URL for ${name}`);

  const upload = await fetch(target.upload_url, {
    method: "PUT",
    headers: { "Content-Type": "application/octet-stream", "Content-Length": `${size}` },
    body: createReadStream(filePath),
    duplex: "half",
  });
  if (!upload.ok) {
    throw new Error(`Uploading ${name} failed: ${upload.status} ${(await upload.text()).trim()}`);
  }
  if (target.verify_url) {
    const verify = await fetch(decodeURIComponent(target.verify_url), {
      method: "POST",
      headers: headers(false),
    });
    if (!verify.ok) {
      throw new Error(`Confirming ${name} failed: ${verify.status} ${(await verify.text()).trim()}`);
    }
  }
  console.log(`Uploaded ${name} (${size} bytes).`);
}

async function uploadWithRetry(releaseId, filePath, overwrite) {
  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    try {
      await uploadAsset(releaseId, filePath, overwrite);
      return;
    } catch (error) {
      if (attempt === MAX_ATTEMPTS) throw error;
      const delay = 2 ** (attempt - 1) * 5000;
      console.warn(`${basename(filePath)}: ${error.message}; retrying in ${delay / 1000}s.`);
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }
}

async function mapWithConcurrency(items, concurrency, worker) {
  let next = 0;
  const run = async () => {
    while (next < items.length) await worker(items[next++]);
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, items.length) }, run));
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const body = args.bodyFile && existsSync(args.bodyFile) ? readFileSync(args.bodyFile, "utf8") : "";
  const release = await ensureRelease({
    tag: args.tag,
    name: args.name,
    body,
    prerelease: args.prerelease,
  });

  const existing = new Set((release.assets || []).map((asset) => asset.name));
  const files = readdirSync(args.assetsDir)
    .map((name) => join(args.assetsDir, name))
    .filter((path) => statSync(path).isFile())
    .sort((left, right) => basename(left).localeCompare(basename(right)));

  const manifest = files.filter((path) => basename(path) === MANIFEST);
  const archives = files.filter((path) => basename(path) !== MANIFEST);
  const toUpload = [...archives, ...manifest].filter((path) => {
    const name = basename(path);
    if (existing.has(name) && !args.overwrite) {
      console.log(`Skipping existing asset ${name}.`);
      return false;
    }
    return true;
  });

  await mapWithConcurrency(
    toUpload.filter((path) => basename(path) !== MANIFEST),
    CONCURRENCY,
    (path) => uploadWithRetry(release.id, path, args.overwrite),
  );
  // The manifest goes last: it points at the archives, so it must never be visible before them.
  for (const path of toUpload.filter((path) => basename(path) === MANIFEST)) {
    await uploadWithRetry(release.id, path, true);
  }

  console.log(
    `CNB release ${args.tag}: https://cnb.cool/${REPOSITORY}/-/releases/tag/${encodeURIComponent(args.tag)}`,
  );
  console.log(
    `Latest manifest: https://cnb.cool/${REPOSITORY}/-/releases/latest/download/${MANIFEST}`,
  );
}

main().catch((error) => {
  console.error(error.message || error);
  process.exitCode = 1;
});
