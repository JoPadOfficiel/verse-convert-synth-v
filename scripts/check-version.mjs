#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const EXPECTED_CARGO_PACKAGE = "verse";
const VERSION_PATTERN =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function fail(message) {
  console.error(`version-check: ${message}`);
  process.exitCode = 1;
}

function readText(relativePath) {
  const absolutePath = path.join(root, relativePath);
  try {
    return fs.readFileSync(absolutePath, "utf8");
  } catch (error) {
    throw new Error(`cannot read ${relativePath}: ${error.message}`);
  }
}

function readJson(relativePath) {
  try {
    return JSON.parse(readText(relativePath));
  } catch (error) {
    throw new Error(`cannot parse ${relativePath}: ${error.message}`);
  }
}

function tomlScalar(block, key) {
  const escapedKey = key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = block.match(
    new RegExp(`^\\s*${escapedKey}\\s*=\\s*["']([^"']+)["']\\s*(?:#.*)?$`, "m"),
  );
  return match?.[1];
}

function cargoManifestPackage(toml) {
  const lines = toml.split(/\r?\n/);
  let inPackage = false;
  const values = {};
  for (const line of lines) {
    const section = line.match(/^\s*\[([^\]]+)\]\s*(?:#.*)?$/);
    if (section) {
      inPackage = section[1] === "package";
      continue;
    }
    if (!inPackage) {
      continue;
    }
    for (const key of ["name", "version"]) {
      const value = tomlScalar(line, key);
      if (value !== undefined) {
        values[key] = value;
      }
    }
  }
  return values;
}

function cargoLockPackages(toml) {
  return toml
    .split(/(?=^\s*\[\[package\]\]\s*$)/m)
    .filter((block) => /^\s*\[\[package\]\]\s*$/m.test(block))
    .map((block) => ({
      name: tomlScalar(block, "name"),
      version: tomlScalar(block, "version"),
    }));
}

function parseArguments(argv) {
  let tag;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--tag") {
      tag = argv[index + 1];
      index += 1;
      if (!tag) {
        throw new Error("--tag requires a value");
      }
    } else if (argument.startsWith("--tag=")) {
      tag = argument.slice("--tag=".length);
      if (!tag) {
        throw new Error("--tag requires a value");
      }
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }
  return { tag };
}

export function latestChangelogRelease(changelog) {
  for (const line of changelog.split(/\r?\n/)) {
    const heading = line.match(
      /^##\s+(?:\[((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))\]|((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)))(?=\s|$)/,
    );
    if (heading) {
      const dateToken = line.match(/\b(\d{4})-(\d{2})-(\d{2})\b/);
      let date;
      if (dateToken) {
        const year = Number(dateToken[1]);
        const month = Number(dateToken[2]);
        const day = Number(dateToken[3]);
        const leapYear =
          year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
        const daysInMonth = [
          31,
          leapYear ? 29 : 28,
          31,
          30,
          31,
          30,
          31,
          31,
          30,
          31,
          30,
          31,
        ];
        if (
          month >= 1 &&
          month <= 12 &&
          day >= 1 &&
          day <= daysInMonth[month - 1]
        ) {
          date = dateToken[0];
        }
      }
      return {
        version: heading[1] ?? heading[2],
        date,
      };
    }
  }
  return {};
}

function main() {
  const { tag } = parseArguments(process.argv.slice(2));
  const packageJson = readJson("package.json");
  const packageLock = readJson("package-lock.json");
  const cargoPackage = cargoManifestPackage(readText("src-tauri/Cargo.toml"));
  const cargoPackages = cargoLockPackages(readText("src-tauri/Cargo.lock"));
  const tauriConfig = readJson("src-tauri/tauri.conf.json");
  const releaseManifest = readJson(".release-please-manifest.json");
  const changelogRelease = latestChangelogRelease(readText("CHANGELOG.md"));

  const version = releaseManifest["."];
  if (typeof version !== "string" || !VERSION_PATTERN.test(version)) {
    fail(
      `.release-please-manifest.json must contain a strict SemVer at key ".": ${String(version)}`,
    );
    return;
  }

  const versions = new Map([
    ["package.json", packageJson.version],
    ["package-lock.json root", packageLock.version],
    ['package-lock.json packages[""]', packageLock.packages?.[""]?.version],
    ["src-tauri/Cargo.toml", cargoPackage.version],
    ["src-tauri/tauri.conf.json", tauriConfig.version],
    [".release-please-manifest.json", version],
    ["CHANGELOG.md latest release", changelogRelease.version],
  ]);

  const matchingCargoPackages = cargoPackages.filter(
    (entry) => entry.name === EXPECTED_CARGO_PACKAGE,
  );
  if (cargoPackage.name !== EXPECTED_CARGO_PACKAGE) {
    fail(
      `src-tauri/Cargo.toml package name is ${JSON.stringify(cargoPackage.name)}; expected ${JSON.stringify(EXPECTED_CARGO_PACKAGE)}`,
    );
  }
  if (matchingCargoPackages.length !== 1) {
    fail(
      `src-tauri/Cargo.lock must contain exactly one [[package]] named ${JSON.stringify(EXPECTED_CARGO_PACKAGE)}; found ${matchingCargoPackages.length}`,
    );
  } else {
    versions.set(
      `src-tauri/Cargo.lock package ${EXPECTED_CARGO_PACKAGE}`,
      matchingCargoPackages[0].version,
    );
  }

  for (const [source, actual] of versions) {
    if (actual !== version) {
      fail(`${source} has version ${JSON.stringify(actual)}; expected ${version}`);
    }
  }

  if (!changelogRelease.date) {
    fail("the latest CHANGELOG.md release heading must include an ISO date");
  }

  if (tag !== undefined) {
    const expectedTag = `v${version}`;
    if (tag !== expectedTag) {
      fail(`tag is ${JSON.stringify(tag)}; expected ${JSON.stringify(expectedTag)}`);
    }
  }

  if (process.exitCode) {
    return;
  }
  console.log(
    `version-check: ${version} is consistent across all release files${tag ? ` and tag ${tag}` : ""}`,
  );
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error));
  }
}
