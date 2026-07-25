#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const EXPECTED_BINARY = "verse";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function fail(message) {
  throw new Error(`bundle-entrypoint-check: ${message}`);
}

function parseArguments(argv) {
  let target;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--target") {
      target = argv[index + 1];
      index += 1;
    } else if (argument.startsWith("--target=")) {
      target = argument.slice("--target=".length);
    } else {
      fail(`unknown argument: ${argument}`);
    }
  }
  if (!target) {
    fail("--target requires a non-empty Rust target triple");
  }
  return target;
}

function cargoPackageValue(toml, key) {
  const packageBlock = toml.match(
    /^\[package\]\s*$([\s\S]*?)(?=^\[[^\]]+\]\s*$|\z)/m,
  )?.[1];
  if (!packageBlock) {
    fail("src-tauri/Cargo.toml has no [package] section");
  }
  const escapedKey = key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return packageBlock.match(
    new RegExp(
      `^\\s*${escapedKey}\\s*=\\s*["']([^"']+)["']\\s*(?:#.*)?$`,
      "m",
    ),
  )?.[1];
}

export function validateIdentityContract({
  cargoName,
  cargoDefaultRun,
  tauriMainBinaryName,
  binaryTargets,
}) {
  if (cargoName !== EXPECTED_BINARY) {
    fail(
      `Cargo package name is ${JSON.stringify(cargoName)}; expected ${JSON.stringify(EXPECTED_BINARY)}`,
    );
  }
  if (cargoDefaultRun !== EXPECTED_BINARY) {
    fail(
      `Cargo default-run is ${JSON.stringify(cargoDefaultRun)}; expected ${JSON.stringify(EXPECTED_BINARY)}`,
    );
  }
  if (tauriMainBinaryName !== EXPECTED_BINARY) {
    fail(
      `Tauri mainBinaryName is ${JSON.stringify(tauriMainBinaryName)}; expected ${JSON.stringify(EXPECTED_BINARY)}`,
    );
  }
  const uniqueTargets = [...new Set(binaryTargets)].sort();
  if (
    uniqueTargets.length !== 1 ||
    uniqueTargets[0] !== EXPECTED_BINARY
  ) {
    fail(
      `Cargo application binaries are ${JSON.stringify(uniqueTargets)}; expected only ${JSON.stringify(EXPECTED_BINARY)}`,
    );
  }
}

export function extractMacBundleExecutable(plist) {
  return plist.match(
    /<key>\s*CFBundleExecutable\s*<\/key>\s*<string>\s*([^<]+?)\s*<\/string>/,
  )?.[1];
}

export function validateArchiveListing(listing) {
  const archivePaths = [...listing.matchAll(/^Path = (.+)$/gm)].map(
    (match) => match[1],
  );
  const executableNames = archivePaths
    .map((entry) => path.win32.basename(entry).toLowerCase())
    .filter((entry) => entry.endsWith(".exe"));
  if (executableNames.includes("corpus_audit.exe")) {
    fail("Windows installer contains the developer-only corpus_audit.exe");
  }
  if (!executableNames.includes(`${EXPECTED_BINARY}.exe`)) {
    fail(
      `Windows installer does not contain ${EXPECTED_BINARY}.exe; executables: ${JSON.stringify(executableNames)}`,
    );
  }
}

function cargoMetadata() {
  const result = spawnSync(
    "cargo",
    [
      "metadata",
      "--manifest-path",
      path.join(root, "src-tauri", "Cargo.toml"),
      "--no-deps",
      "--format-version",
      "1",
    ],
    { encoding: "utf8" },
  );
  if (result.status !== 0) {
    fail(`cargo metadata failed: ${(result.stderr || result.stdout).trim()}`);
  }
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    fail(`cargo metadata returned invalid JSON: ${error.message}`);
  }
}

function releaseDirectory(target) {
  return path.join(root, "src-tauri", "target", target, "release");
}

function verifyBuiltExecutable(releaseRoot, target) {
  const extension = target.includes("windows") ? ".exe" : "";
  const executable = path.join(releaseRoot, `${EXPECTED_BINARY}${extension}`);
  if (!fs.existsSync(executable)) {
    fail(`built application executable is missing: ${executable}`);
  }
}

function verifyMacBundle(releaseRoot, productName) {
  const app = path.join(releaseRoot, "bundle", "macos", `${productName}.app`);
  const plistPath = path.join(app, "Contents", "Info.plist");
  if (!fs.existsSync(plistPath)) {
    fail(`macOS bundle plist is missing: ${plistPath}`);
  }
  const actual = extractMacBundleExecutable(fs.readFileSync(plistPath, "utf8"));
  if (actual !== EXPECTED_BINARY) {
    fail(
      `macOS CFBundleExecutable is ${JSON.stringify(actual)}; expected ${JSON.stringify(EXPECTED_BINARY)}`,
    );
  }
  const executable = path.join(app, "Contents", "MacOS", EXPECTED_BINARY);
  if (!fs.existsSync(executable)) {
    fail(`macOS bundle executable is missing: ${executable}`);
  }
}

function verifyWindowsBundle(releaseRoot) {
  const nsisDirectory = path.join(releaseRoot, "bundle", "nsis");
  const installers = fs.existsSync(nsisDirectory)
    ? fs
        .readdirSync(nsisDirectory)
        .filter((entry) => entry.toLowerCase().endsWith(".exe"))
        .map((entry) => path.join(nsisDirectory, entry))
    : [];
  if (installers.length !== 1) {
    fail(
      `expected exactly one NSIS installer under ${nsisDirectory}; found ${installers.length}`,
    );
  }
  const result = spawnSync("7z", ["l", "-slt", installers[0]], {
    encoding: "utf8",
  });
  if (result.status !== 0) {
    fail(
      `7z could not inspect ${installers[0]}: ${(result.stderr || result.stdout).trim()}`,
    );
  }
  validateArchiveListing(result.stdout);
}

function main() {
  const target = parseArguments(process.argv.slice(2));
  const cargoToml = fs.readFileSync(
    path.join(root, "src-tauri", "Cargo.toml"),
    "utf8",
  );
  const tauriConfig = JSON.parse(
    fs.readFileSync(path.join(root, "src-tauri", "tauri.conf.json"), "utf8"),
  );
  const metadata = cargoMetadata();
  const versePackage = metadata.packages.find(
    (candidate) => candidate.name === EXPECTED_BINARY,
  );
  if (!versePackage) {
    fail(`cargo metadata has no ${EXPECTED_BINARY} package`);
  }
  const binaryTargets = versePackage.targets
    .filter((candidate) => candidate.kind.includes("bin"))
    .map((candidate) => candidate.name);
  validateIdentityContract({
    cargoName: cargoPackageValue(cargoToml, "name"),
    cargoDefaultRun: cargoPackageValue(cargoToml, "default-run"),
    tauriMainBinaryName: tauriConfig.mainBinaryName,
    binaryTargets,
  });

  const releaseRoot = releaseDirectory(target);
  verifyBuiltExecutable(releaseRoot, target);
  if (target.endsWith("apple-darwin")) {
    verifyMacBundle(releaseRoot, tauriConfig.productName);
  } else if (target.includes("windows")) {
    verifyWindowsBundle(releaseRoot);
  }
  console.log(
    `bundle-entrypoint-check: ${target} packages ${EXPECTED_BINARY} as the application entry point`,
  );
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
