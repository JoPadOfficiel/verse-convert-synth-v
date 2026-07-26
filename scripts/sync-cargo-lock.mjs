#!/usr/bin/env node

// Release Please bumps every manifest it is configured for, but its TOML
// updater cannot address the `verse` entry inside src-tauri/Cargo.lock: that
// file is an array of [[package]] tables, and the jsonpath filter needed to
// select one of them is not supported. The locked version therefore stays on
// the previous release while everything else moves, check-version.mjs refuses
// the mismatch, and build.yml runs that check before compiling — so the release
// publishes with no binaries at all.
//
// This script performs that one edit: it copies the version declared in
// src-tauri/Cargo.toml onto the locked `verse` package. It touches nothing
// else, needs no Rust toolchain, and is a no-op once the two agree.

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const CARGO_PACKAGE = "verse";
const MANIFEST = "src-tauri/Cargo.toml";
const LOCKFILE = "src-tauri/Cargo.lock";
const VERSION_PATTERN = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function readText(relativePath) {
  try {
    return fs.readFileSync(path.join(root, relativePath), "utf8");
  } catch (error) {
    throw new Error(`cannot read ${relativePath}: ${error.message}`);
  }
}

/** Value of a quoted scalar key, ignoring trailing comments. */
function tomlScalar(block, key) {
  const match = block.match(
    new RegExp(`^\\s*${key}\\s*=\\s*["']([^"']+)["']\\s*(?:#.*)?$`, "m"),
  );
  return match?.[1];
}

function manifestVersion(toml) {
  const lines = toml.split(/\r?\n/);
  let inPackage = false;
  for (const line of lines) {
    const section = line.match(/^\s*\[([^\]]+)\]\s*(?:#.*)?$/);
    if (section) {
      inPackage = section[1] === "package";
      continue;
    }
    if (!inPackage) {
      continue;
    }
    const version = tomlScalar(line, "version");
    if (version !== undefined) {
      return version;
    }
  }
  throw new Error(`${MANIFEST} declares no [package] version`);
}

/**
 * Rewrites the version of the `[[package]]` table named `CARGO_PACKAGE`.
 * Splitting on the table header keeps every other byte of the file untouched,
 * so the result stays exactly what Cargo would have written.
 */
export function retargetLockedVersion(lock, version) {
  const blocks = lock.split(/(?=^\[\[package\]\]$)/m);
  const matches = blocks
    .map((block, index) => ({ block, index }))
    .filter(({ block }) => tomlScalar(block, "name") === CARGO_PACKAGE);
  if (matches.length === 0) {
    throw new Error(`${LOCKFILE} has no [[package]] named ${CARGO_PACKAGE}`);
  }
  if (matches.length > 1) {
    throw new Error(
      `${LOCKFILE} has ${matches.length} [[package]] entries named ${CARGO_PACKAGE}`,
    );
  }
  const { block, index } = matches[0];
  const current = tomlScalar(block, "version");
  if (current === undefined) {
    throw new Error(`${LOCKFILE} package ${CARGO_PACKAGE} declares no version`);
  }
  if (current === version) {
    return { changed: false, from: current };
  }
  blocks[index] = block.replace(
    /^(\s*version\s*=\s*)["'][^"']+["']/m,
    `$1"${version}"`,
  );
  return { changed: true, from: current, text: blocks.join("") };
}

function main() {
  const version = manifestVersion(readText(MANIFEST));
  if (!VERSION_PATTERN.test(version)) {
    throw new Error(`${MANIFEST} declares an unsupported version: ${version}`);
  }
  const lock = readText(LOCKFILE);
  const result = retargetLockedVersion(lock, version);
  if (!result.changed) {
    console.log(`sync-cargo-lock: ${CARGO_PACKAGE} is already ${version}`);
    return;
  }
  fs.writeFileSync(path.join(root, LOCKFILE), result.text);
  console.log(
    `sync-cargo-lock: ${CARGO_PACKAGE} ${result.from} -> ${version} in ${LOCKFILE}`,
  );
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(
      `sync-cargo-lock: ${error instanceof Error ? error.message : String(error)}`,
    );
    process.exitCode = 1;
  }
}
