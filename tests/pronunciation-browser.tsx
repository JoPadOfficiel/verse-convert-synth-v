// Browser-only regression. React, the App, Settings, storage helpers and Tauri
// adapter are real; only native IPC/dialogs are substituted. No extra test deps.
import React, { act } from "react";
import { createRoot } from "react-dom/client";
import App from "../src/App";
import { ThemeProvider } from "../src/components/theme-provider";
import type { FileResult, PronunciationProfile } from "../src/lib/tauri";
import "../src/index.css";

const profiles: PronunciationProfile[] = ["frenchMillefeuille", "englishArpabet", "default"];
const key = "verse.pronunciationProfile";
const runKey = "verse.pronunciationBrowserTest";
type TestResult = { name: string; passed: true };
type Progress = { index: number; restart: boolean; tests: TestResult[]; original: string | null };
let progress: Progress | null = JSON.parse(sessionStorage.getItem(runKey) ?? "null");
let mode: "accept" | "reject" | "pending" = "accept";
let finishPending: (() => void) | undefined;
const calls: { command: string; payload: Record<string, unknown> }[] = [];
const root = createRoot(document.getElementById("root")!);
const results = document.getElementById("test-results")!;
const runButton = document.getElementById("run-tests") as HTMLButtonElement;

function check(ok: unknown, message: string): asserts ok {
  if (!ok) throw new Error(message);
}
function record(name: string) {
  progress!.tests.push({ name, passed: true });
  results.textContent = `${progress!.tests.length} checks passed. ${name}`;
}
function fixture(profile: unknown): FileResult {
  return {
    path: "/test/song.mscz", name: `Accepted ${profile}`, ok: true, error: null, msg: null,
    nParts: 1, nVoices: 1, nTracks: 1, placed: 1, tracks: [],
    parts: [{ sourceId: "part", part: "Voice", staves: 1, voices: 1, trackIds: [0],
      vocalCandidateTrackIds: [0], sourceTrackIds: ["track"], notes: 1, placed: 1,
      sourceRole: "vocal", exportRepresentation: "vocalNotes", requiresVoiceAssignment: true,
      hasAudioStem: false, lyricStatus: { state: "sourceOwned", sourceTextCount: 1,
        projectedTextCount: 1, explicitEmptyCount: 0, continuationCount: 0, unsupportedCount: 0 },
      warnings: [{ code: "TEST_VERDICT", severity: "info", message: `Verdict ${profile}`, sourceId: "note" }] }],
    audioStatus: { state: "notRendered" }, requiresVoiceAssignment: true,
    bundleReady: true, warnings: [], out: null,
  };
}

Object.assign(window, {
  IS_REACT_ACT_ENVIRONMENT: true,
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
  __TAURI_INTERNALS__: {
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    transformCallback: (() => { let id = 0; return () => ++id; })(),
    unregisterCallback() {},
    async invoke(command: string, payload: Record<string, unknown>) {
      calls.push({ command, payload });
      switch (command) {
        case "plugin:event|listen": return calls.length;
        case "plugin:event|unlisten": return null;
        case "plugin:window|inner_size": return { width: 1000, height: 760 };
        case "plugin:window|scale_factor": return 1;
        case "renderer_status": return { state: "missing", configured: false, provider: null,
          version: null, fullScoreMix: false, message: "Browser test: native renderer is mocked." };
        case "plugin:dialog|open": return ["/test/song.mscz"];
        case "plugin:dialog|save": return "/test/export.ustx";
        case "export_svp": return "/test/export.ustx";
        case "convert_files":
          if (mode === "reject") throw new Error("Injected reanalysis rejection");
          if (mode === "pending") await new Promise<void>((resolve) => { finishPending = resolve; });
          return [fixture(payload.pronunciationProfile)];
        default: throw new Error(`Unexpected native command: ${command}`);
      }
    },
  },
});

async function mount() {
  await act(async () => { root.render(<React.StrictMode><ThemeProvider><App /></ThemeProvider></React.StrictMode>); });
}
function header() {
  const select = document.querySelector<HTMLSelectElement>('header select[aria-label="OpenUtau pronunciation"]');
  check(select, "Pronunciation selector must be in the header");
  check(select.closest("header")!.querySelector('button[title="Theme"]'), "Theme must remain beside pronunciation");
  return select;
}
function button(text: string) {
  const found = [...document.querySelectorAll<HTMLButtonElement>("#root button")]
    .find((b) => b.textContent?.trim().startsWith(text) || b.title === text);
  check(found, `Missing button: ${text}`);
  return found;
}
async function click(text: string) {
  const element = button(text);
  check(!element.disabled, `Disabled button: ${text}`);
  await act(async () => { element.click(); });
}
async function choose(profile: PronunciationProfile, select = header()) {
  check(!select.disabled, "Profile control must be enabled before a change");
  await act(async () => { select.value = profile; select.dispatchEvent(new Event("change", { bubbles: true })); });
}
function last(command: string) {
  let call: (typeof calls)[number] | undefined;
  for (let index = calls.length - 1; index >= 0; index -= 1) {
    if (calls[index].command === command) {
      call = calls[index];
      break;
    }
  }
  check(call, `No ${command} command recorded`);
  return call.payload;
}
function stored(profile: PronunciationProfile) {
  check(localStorage.getItem(key) === profile, `Stored choice must be ${profile}`);
}
async function synced(profile: PronunciationProfile) {
  await click("Settings");
  const select = document.querySelector<HTMLSelectElement>("#pronunciation-profile")!;
  check(select?.value === profile && header().value === profile, "Header and Settings must agree");
  await click("Back");
}

function cleanupRun(original: string | null | undefined) {
  sessionStorage.removeItem(runKey);
  if (original !== undefined) {
    if (original === null) localStorage.removeItem(key);
    else localStorage.setItem(key, original);
  }
  progress = null;
  mode = "accept";
  finishPending = undefined;
  calls.length = 0;
  runButton.disabled = false;
}

async function run() {
  runButton.disabled = true;
  try {
    if (!progress) {
      calls.length = 0;
      mode = "accept";
      finishPending = undefined;
      progress = { index: 0, restart: false, tests: [], original: localStorage.getItem(key) };
      localStorage.removeItem(key);
      await act(async () => { root.render(null); });
      await mount();
    }
    const profile = profiles[progress.index];
    if (!progress.restart) {
      await choose(profile);
      stored(profile);
      check(!calls.some((c) => c.command === "convert_files"), "Empty selection must not analyse files");
      await synced(profile);
      record(`${profile}: empty choice and synchronized controls`);
      progress.restart = true;
      sessionStorage.setItem(runKey, JSON.stringify(progress));
      location.reload();
      return;
    }
    check(header().value === profile, "Fresh document must restore the visible profile");
    stored(profile);
    record(`${profile}: full page restart restores preference`);
    await click("Drop your files, or click to choose");
    check(last("convert_files").pronunciationProfile === profile, "Next import must use saved profile");
    await click("Vocals only");
    check(last("export_svp").pronunciationProfile === profile, "Next export must use saved profile");
    record(`${profile}: post-restart import and export propagation`);

    const next = profiles[(progress.index + 1) % profiles.length];
    mode = "pending";
    await choose(next);
    stored(profile);
    check(header().disabled && header().value === profile, "Pending change must keep old visible choice and disable input");
    await click("Settings");
    const settings = document.querySelector<HTMLSelectElement>("#pronunciation-profile")!;
    check(settings.disabled && settings.value === profile, "Pending Settings selection must be disabled and unchanged");
    const count = calls.filter((c) => c.command === "convert_files").length;
    await act(async () => { settings.dispatchEvent(new Event("change", { bubbles: true })); });
    check(calls.filter((c) => c.command === "convert_files").length === count, "Busy callback cannot start another command");
    stored(profile);
    check(finishPending, "Pending native command must exist");
    mode = "accept";
    await act(async () => { finishPending!(); });
    check(header().value === next && settings.value === next, "Accepted loaded choice must synchronize");
    stored(next);
    record(`${profile}: pending/busy choice commits only after success`);

    mode = "reject";
    await choose(profile, settings);
    stored(next);
    check(header().value === next && settings.value === next, "Rejected choice must revert both controls");
    check(document.querySelector('[role="alert"]')?.textContent?.includes("Injected reanalysis rejection"), "Rejection must remain visible");
    await click("Back");
    await click(`Accepted ${next}`);
    check(document.getElementById("root")!.textContent!.includes(`Verdict ${next}`), "Rejection must preserve prior diagnostics");
    record(`${profile}: rejection preserves preference, results and diagnostics`);

    mode = "accept";
    await click("Synthesizer V");
    stored(next);
    check(last("convert_files").exportTarget === "svp", "SVP target must reach reanalysis");
    await click("Settings");
    const disabled = document.querySelector<HTMLSelectElement>("#pronunciation-profile")!;
    check(disabled.disabled && disabled.value === next, "SVP must retain but disable the USTX choice");
    await click("Back");
    await click("OpenUtau");
    check(header().value === next, "Returning to USTX must restore the chosen profile");
    record(`${profile}: target roundtrip preserves the USTX preference`);

    await click("Clear");
    await act(async () => { root.render(null); });
    await mount();
    check(header().value === next, "Accepted loaded-file preference must survive a fresh App mount");
    stored(next);
    record(`${profile}: accepted loaded preference survives remount after clearing files`);
    progress.index += 1;
    progress.restart = false;
    if (progress.index < profiles.length) {
      calls.length = 0;
      await run();
      return;
    }
    const setItem = Storage.prototype.setItem;
    try {
      Storage.prototype.setItem = function(k, value) {
        if (k === key) throw new Error("Storage writes unavailable");
        return setItem.call(this, k, value);
      };
      const writeFailureProfile: PronunciationProfile =
        header().value === "englishArpabet" ? "frenchMillefeuille" : "englishArpabet";
      await choose(writeFailureProfile);
      check(header().value === writeFailureProfile, "Unavailable storage must leave current session usable");
      record("Storage write failure preserves current-session usability");
    } finally { Storage.prototype.setItem = setItem; }
    try {
      const themeButton = button("Theme");
      const before = themeButton.innerHTML;
      Storage.prototype.setItem = function(k, value) {
        if (k === "verse-theme") throw new Error("Theme storage writes unavailable");
        return setItem.call(this, k, value);
      };
      await click("Theme");
      check(themeButton.innerHTML !== before, "Theme must still change when its storage write fails");
      check(
        document.documentElement.classList.contains("light") ||
          document.documentElement.classList.contains("dark"),
        "Theme fallback must leave a rendered light/dark class",
      );
      record("Theme write failure preserves current-session theme changes");
    } finally { Storage.prototype.setItem = setItem; }
    const getItem = Storage.prototype.getItem;
    try {
      Storage.prototype.getItem = () => { throw new Error("Storage reads unavailable"); };
      await act(async () => { root.render(null); });
      await mount();
      check(header().value === "default", "Unavailable storage reads must fall back to Default");
      await choose("englishArpabet");
      check(header().value === "englishArpabet", "Storage read failure must not prevent session choices");
      record("Storage read failure preserves startup and current-session usability");
    } finally { Storage.prototype.getItem = getItem; }
    const result = { passed: true, tests: progress.tests, userAgent: navigator.userAgent,
      boundary: "Real browser, React StrictMode, App, Settings, storage and Tauri adapter; native IPC mocked", reloads: profiles.length };
    const response = await fetch("/__pronunciation-results", { method: "POST", body: JSON.stringify(result) });
    check(response.ok, "Could not persist browser test receipt");
    results.textContent = JSON.stringify(result, null, 2);
    cleanupRun(progress.original);
  } catch (error) {
    const original = progress?.original;
    const result = { passed: false, tests: progress?.tests ?? [], error: String(error), stack: error instanceof Error ? error.stack : null };
    results.textContent = JSON.stringify(result, null, 2);
    try {
      await fetch("/__pronunciation-results", { method: "POST", body: JSON.stringify(result) });
    } catch {
      // Reporting failure must not strand the test profile or disabled controls.
    } finally {
      cleanupRun(original);
    }
  }
}
await mount();
runButton.onclick = () => void run();
if (progress) void run();
