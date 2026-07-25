import assert from "node:assert/strict";
import test from "node:test";

import {
  extractMacBundleExecutable,
  validateArchiveListing,
  validateIdentityContract,
} from "../scripts/check-bundle-entrypoint.mjs";

const validIdentity = {
  cargoName: "verse",
  cargoDefaultRun: "verse",
  tauriMainBinaryName: "verse",
  binaryTargets: ["verse"],
};

test("release identity accepts only the explicit Verse application binary", () => {
  assert.doesNotThrow(() => validateIdentityContract(validIdentity));
});

test("release identity rejects a missing or auxiliary default binary", () => {
  for (const cargoDefaultRun of [undefined, "corpus_audit"]) {
    assert.throws(
      () =>
        validateIdentityContract({
          ...validIdentity,
          cargoDefaultRun,
        }),
      /Cargo default-run/,
    );
  }

  assert.throws(
    () =>
      validateIdentityContract({
        ...validIdentity,
        binaryTargets: ["corpus_audit", "verse"],
      }),
    /expected only "verse"/,
  );
});

test("release identity rejects a wrong Tauri main binary", () => {
  assert.throws(
    () =>
      validateIdentityContract({
        ...validIdentity,
        tauriMainBinaryName: "corpus_audit",
      }),
    /Tauri mainBinaryName/,
  );
});

test("macOS plist extraction distinguishes Verse from corpus audit", () => {
  const plist = (executable) => `
    <plist><dict>
      <key>CFBundleExecutable</key>
      <string>${executable}</string>
    </dict></plist>
  `;

  assert.equal(extractMacBundleExecutable(plist("verse")), "verse");
  assert.equal(
    extractMacBundleExecutable(plist("corpus_audit")),
    "corpus_audit",
  );
  assert.equal(extractMacBundleExecutable("<plist><dict/></plist>"), undefined);
});

test("Windows archive listing requires verse.exe and rejects corpus audit", () => {
  assert.doesNotThrow(() =>
    validateArchiveListing(
      ["Path = installer.exe", "Path = app\\\\verse.exe"].join("\n"),
    ),
  );
  assert.throws(
    () =>
      validateArchiveListing(
        [
          "Path = installer.exe",
          "Path = app\\\\verse.exe",
          "Path = tools\\\\corpus_audit.exe",
        ].join("\n"),
      ),
    /developer-only corpus_audit\.exe/,
  );
  assert.throws(
    () =>
      validateArchiveListing(
        ["Path = installer.exe", "Path = app\\\\corpus_audit.exe"].join("\n"),
      ),
    /corpus_audit\.exe/,
  );
});
