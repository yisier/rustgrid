#!/usr/bin/env node
// Generate the updater's static release manifest, the file the app reads through
// `self_update::backends::manifest` (schema 1) instead of GitHub's rate-limited REST API.
//
//   node tools/release/make-manifest.mjs --dir dist --tag v0.1.19 --version 0.1.19 \
//     --base-url https://github.com/yisier/rustgrid/releases/download/v0.1.19/ \
//     --out dist/latest.json
//
// Only the portable archives are listed: `self_update` picks one by the `PLATFORM` substring in its
// name (see `crates/rustgrid-app/src/app/update.rs`) and verifies the `sha256` digest before it
// installs anything. Run it again with the CNB base URL to mint the mirror copy (see
// `.github/workflows/sync-cnb.yml`).

import { createHash } from "node:crypto";
import { createReadStream, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const ASSET = /^RustGrid-.*\.(zip|tar\.gz)$/i;

function parseArgs(argv) {
  const args = { dir: "", tag: "", version: "", baseUrl: "", out: "", date: "" };
  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index];
    if (arg === "--dir") args.dir = argv[++index];
    else if (arg === "--tag") args.tag = argv[++index];
    else if (arg === "--version") args.version = argv[++index];
    else if (arg === "--base-url") args.baseUrl = argv[++index];
    else if (arg === "--out") args.out = argv[++index];
    else if (arg === "--date") args.date = argv[++index];
    else throw new Error(`Unknown argument: ${arg}`);
  }
  for (const key of ["dir", "tag", "version", "baseUrl", "out"]) {
    if (!args[key]) throw new Error(`Missing --${key.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`);
  }
  if (!args.baseUrl.endsWith("/")) args.baseUrl += "/";
  return args;
}

function sha256(path) {
  return new Promise((resolve, reject) => {
    const hash = createHash("sha256");
    createReadStream(path)
      .on("error", reject)
      .on("data", (chunk) => hash.update(chunk))
      .on("end", () => resolve(hash.digest("hex")));
  });
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const names = readdirSync(args.dir)
    .filter((name) => ASSET.test(name) && statSync(join(args.dir, name)).isFile())
    .sort((left, right) => left.localeCompare(right));
  if (names.length === 0) throw new Error(`No RustGrid archives in ${args.dir}`);

  const version = args.version.trim().replace(/^v/, "");
  if (version !== args.tag.trim().replace(/^v/, "")) {
    throw new Error(`--version ${args.version} does not match tag ${args.tag}`);
  }

  const assets = [];
  for (const name of names) {
    const path = join(args.dir, name);
    assets.push({
      name,
      url: `${args.baseUrl}${name}`,
      digest: `sha256:${await sha256(path)}`,
    });
  }

  const manifest = {
    schema: 1,
    releases: [
      {
        version,
        date: args.date || new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
        assets,
      },
    ],
  };
  writeFileSync(args.out, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`Wrote ${args.out} (${assets.length} assets for ${version})`);
  for (const asset of assets) console.log(`  ${asset.name}  ${asset.digest.slice(7, 19)}…`);
}

main().catch((error) => {
  console.error(error.message || error);
  process.exitCode = 1;
});
