// Browser-only regression. React, the App, Settings, storage helpers and Tauri
// adapter are real; only native IPC/dialogs are substituted. No extra test deps.
import React, { act } from "react";
import { createRoot } from "react-dom/client";
import App from "../src/App";
import { ThemeProvider } from "../src/components/theme-provider";
import type { FileResult, ImportedCorrection, PronunciationMemory, PronunciationProfile } from "../src/lib/tauri";
import packageMetadata from "../package.json";
import "../src/index.css";

const profiles: PronunciationProfile[] = ["automatic", "frenchMillefeuille", "englishArpabet", "spanishDiffSinger", "portugueseDiffSinger", "default"];
const key = "verse.pronunciationProfile";
const runKey = "verse.pronunciationBrowserTest";
type TestResult = { name: string; passed: true };
type Progress = { index: number; restart: boolean; tests: TestResult[]; original: string | null };
let progress: Progress | null = JSON.parse(sessionStorage.getItem(runKey) ?? "null");
let mode: "accept" | "reject" | "pending" = "accept";
let finishPending: (() => void) | undefined;
let correctedDialogPath: string | null = "/test/choir-edited.ustx";
let comparisonMode: "accept" | "reject" | "pending" = "accept";
let finishComparison: (() => void) | undefined;
const callbacks = new Map<number, (event: unknown) => void>();
const listeners = new Map<number, { event: string; handler: number }>();
let listenerId = 0;
let delayListenerCompletion = false;
const pendingListenerCompletions: (() => void)[] = [];
let comparisonProposals: ImportedCorrection[] = [];
const nativeVersion = "7.8.9-native-test";
let versionMode: "accept" | "reject" | "pending" = "accept";
const pendingVersions: { resolve: (version: string) => void; reject: (error: Error) => void }[] = [];
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
    bundleReady: true, warnings: [], out: null, pronunciationSnapshotId: "browser-frozen-plan",
  };
}

Object.assign(window, {
  IS_REACT_ACT_ENVIRONMENT: true,
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
  __TAURI_INTERNALS__: {
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    transformCallback: (() => { let id = 0; return (callback: (event: unknown) => void) => { callbacks.set(++id, callback); return id; }; })(),
    unregisterCallback(id: number) { callbacks.delete(id); },
    async invoke(command: string, payload: Record<string, unknown>) {
      calls.push({ command, payload });
      switch (command) {
        case "plugin:app|version":
          if (versionMode === "reject") throw new Error("Injected native version rejection");
          if (versionMode === "pending") return new Promise<string>((resolve, reject) => { pendingVersions.push({ resolve, reject }); });
          return nativeVersion;
        case "pronunciation_memory": { const memory: PronunciationMemory = {
          baseline: "verse-lingua-lexical-context-v1", references: [{ id: "other-reference", exportSha256: "other-export-hash", sourceLabel: "Other score.mscz", exportLabel: "Other score.ustx", createdAtUnixSeconds: 1791190000 }, { id: "export-reference", exportSha256: "source-export-hash", sourceLabel: "Choir original.mscz", exportLabel: "Choir reviewed export.ustx", createdAtUnixSeconds: 1791190000 }],
          history: [{ status: "pending", correction: {
            id: "imported-correction", fingerprint: "fingerprint", scope: "compatible_context", voice: { singer: "Reviewed singer", inventory_sha256: "duration-and-acoustic-inventory", configuration_sha256: "voice-configuration" },
            word: { id: "word", key: "ciel", original: ["ciel"], members: ["word"], context: ["le", "ciel", "bleu"], context_target: 1, attacks: 1, manual: false, owner: { track: "track", part: "P1", staff: "1", voice: "2", occurrence: 3, segment: 1, lane: "lyrics", verse: 2 } },
            before: { language: "en", phonemizer: "DiffSinger English Phonemizer", lexical_reading: "ciel", phones: ["en/s", "en/iy", "en/l"], alphabet: "cmu39" },
            after: { language: "fr", phonemizer: "DiffSinger French Millefeuille Phonemizer", lexical_reading: "ciel", phones: ["fr/s", "fr/y", "fr/ae", "fr/l"], alphabet: "millefeuille" },
            provenance: { source_sha256: "source-hash", export_sha256: "export-hash", corrected_sha256: "corrected-hash", confirmed_after_listening: true, symbol_validation: "unknown", policy: "verse-pronunciation-v1", observed_singer: "Observed singer only" },
          } }],
        }; comparisonProposals = [{ ...memory.history[0].correction, id: "reviewed-correction", scope: "occurrence_only" }]; return memory; }
        case "pronunciation_set_status": return null;
        case "pronunciation_confirm": return null;
        case "pronunciation_compare":
          if (comparisonMode === "reject") throw new Error("Injected comparison rejection");
          if (comparisonMode === "pending") await new Promise<void>((resolve) => { finishComparison = resolve; });
          return { reference_id: payload.referenceId, corrected_sha256: "edited-hash", proposals: comparisonProposals, diagnostics: [] };
        case "pronunciation_release_snapshots": return null;
        case "plugin:event|listen": {
          const id = ++listenerId;
          listeners.set(id, { event: payload.event as string, handler: payload.handler as number });
          if (delayListenerCompletion && payload.event === "tauri://drag-leave") await new Promise<void>((resolve) => { pendingListenerCompletions.push(resolve); });
          return id;
        }
        case "plugin:event|unlisten": listeners.delete(payload.eventId as number); return null;
        case "plugin:window|inner_size": return { width: 1000, height: 760 };
        case "plugin:window|scale_factor": return 1;
        case "renderer_status": return { state: "missing", configured: false, provider: null,
          version: null, fullScoreMix: false, message: "Browser test: native renderer is mocked." };
        case "plugin:dialog|open": return (payload.options as { multiple: boolean }).multiple ? ["/test/song.mscz"] : correctedDialogPath;
        case "plugin:dialog|save": return "/test/export.ustx";
        case "pronunciation_export_svp": return "/test/export.ustx";
        case "pronunciation_convert_files":
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
  const select = document.querySelector<HTMLSelectElement>('header select[aria-label="Pronunciation"]');
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
async function drop(paths: string[]) {
  await act(async () => {
    for (const [id, listener] of [...listeners]) {
      if (listener.event === "tauri://drag-drop") callbacks.get(listener.handler)?.({ id, event: listener.event, payload: { paths, position: { x: 100, y: 100 } } });
    }
  });
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
function versionFooter(message: string) {
  const footer = document.querySelector<HTMLElement>('footer[aria-label="Application version"]');
  check(footer?.textContent?.trim() === message, `Settings version must display ${message}`);
  check(footer.parentElement?.lastElementChild === footer, "Version footer must be the last Settings element");
  const previous = footer.previousElementSibling;
  check(previous && previous.textContent?.includes("A complete .versebundle"), "Version must follow the bundle explanation");
  const bounds = footer.getBoundingClientRect();
  check(bounds.width > 0 && bounds.height > 0 && bounds.top >= previous.getBoundingClientRect().bottom,
    "Version footer must render below the final Settings content");
  return footer;
}
async function synced(profile: PronunciationProfile) {
  if (profile === "automatic") versionMode = "pending";
  await click("Settings");
  const select = document.querySelector<HTMLSelectElement>("#pronunciation-profile")!;
  check(select?.value === profile && header().value === profile, "Header and Settings must agree");
  if (profile === "automatic") {
    versionFooter("Loading version…");
    check(!select.disabled, "Loading the version must leave Settings enabled");
    check(pendingVersions.length > 0, "Settings must request the native app version");
    versionMode = "accept";
    await act(async () => { for (const request of pendingVersions.splice(0)) request.resolve(nativeVersion); });
  }
  check(nativeVersion !== packageMetadata.version, "Native version fixture must differ from the web package version");
  versionFooter(`Verse version ${nativeVersion}`);
  check(calls.some((call) => call.command === "plugin:app|version"), "Version must come from the native app getter");
  await click("Back");
  if (profile === "automatic") {
    versionMode = "reject";
    await click("Settings");
    versionFooter("Version unavailable");
    const availableSelect = document.querySelector<HTMLSelectElement>("#pronunciation-profile")!;
    const renderer = document.querySelector<HTMLInputElement>("#renderer-path")!;
    check(!availableSelect.disabled && availableSelect.value === profile && !renderer.disabled,
      "Version rejection must preserve editable settings and the selected profile");
    check(!document.querySelector('[role="alert"]'), "Version rejection must stay in the footer");
    const theme = document.documentElement.classList.contains("light") ? "Dark" : "Light";
    await click(theme);
    check(document.documentElement.classList.contains(theme.toLowerCase()), "Appearance must remain usable after version rejection");
    await click("Back");

    versionMode = "pending";
    await click("Settings");
    const oldFooter = versionFooter("Loading version…");
    const staleRequests = pendingVersions.splice(0);
    check(staleRequests.length > 0, "Unmount regression needs an unresolved native version request");
    await click("Back");
    check(!oldFooter.isConnected, "Closing Settings must unmount the version footer");
    versionMode = "accept";
    await click("Settings");
    await act(async () => {
      for (let index = 0; index < staleRequests.length; index += 1) {
        if (index % 2 === 0) staleRequests[index].resolve("stale-native-version");
        else staleRequests[index].reject(new Error("Late native version rejection"));
      }
    });
    versionFooter(`Verse version ${nativeVersion}`);
    check(oldFooter.textContent?.trim() === "Loading version…", "Late native results must not update the unmounted footer");
    check(document.querySelector<HTMLSelectElement>("#pronunciation-profile")?.value === profile,
      "Late native results must preserve reopened Settings");
    await click("Back");
  }
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
  versionMode = "accept";
  pendingVersions.length = 0;
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
      check(header().value === "automatic", "Fresh startup must select Automatic FR+EN+ES+PT");
      check(header().selectedOptions[0].textContent?.includes("ES + PT"), "Automatic must advertise all four languages");
      await click("Drop your files, or click to choose");
      check(last("pronunciation_convert_files").pronunciationProfile === "automatic", "First import needs no manual language selection");
      await click("Clear");
      calls.length = 0;
      await choose("default");
      record("Fresh startup imports automatically in four-language mode");
    }
    const profile = profiles[progress.index];
    if (!progress.restart) {
      await choose(profile);
      stored(profile);
      check(!calls.some((c) => c.command === "pronunciation_convert_files"), "Empty selection must not analyse files");
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
    check(last("pronunciation_convert_files").pronunciationProfile === profile, "Next import must use saved profile");
    await click("Vocals only");
    check(last("pronunciation_export_svp").pronunciationProfile === profile, "Next export must use saved profile");
    record(`${profile}: post-restart import and export propagation`);

    const next = profiles[(progress.index + 1) % profiles.length];
    mode = "pending";
    await choose(next);
    stored(profile);
    check(header().disabled && header().value === profile, "Pending change must keep old visible choice and disable input");
    await click("Settings");
    const settings = document.querySelector<HTMLSelectElement>("#pronunciation-profile")!;
    check(settings.disabled && settings.value === profile, "Pending Settings selection must be disabled and unchanged");
    const count = calls.filter((c) => c.command === "pronunciation_convert_files").length;
    await act(async () => { settings.dispatchEvent(new Event("change", { bubbles: true })); });
    check(calls.filter((c) => c.command === "pronunciation_convert_files").length === count, "Busy callback cannot start another command");
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
    check(last("pronunciation_convert_files").exportTarget === "svp", "SVP target must reach reanalysis");
    await click("Settings");
    const svpProfile = document.querySelector<HTMLSelectElement>("#pronunciation-profile")!;
    check(!svpProfile.disabled && svpProfile.value === next, "SVP must expose the target-neutral Automatic/Default choice");
    if (next === "automatic") {
      await choose("default", svpProfile);
      check(last("pronunciation_convert_files").exportTarget === "svp" && last("pronunciation_convert_files").pronunciationProfile === "default",
        "SVP Default selection must reanalyse as SVP");
      await choose("automatic", svpProfile);
      check(last("pronunciation_convert_files").exportTarget === "svp" && last("pronunciation_convert_files").pronunciationProfile === "automatic",
        "SVP Automatic FR+EN+ES+PT selection must reanalyse as SVP");
      stored("automatic");
    }
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
      check(header().value === "automatic", "Unavailable storage reads must fall back to Automatic");
      await choose("englishArpabet");
      check(header().value === "englishArpabet", "Storage read failure must not prevent session choices");
      record("Storage read failure preserves startup and current-session usability");
    } finally { Storage.prototype.getItem = getItem; }
    await click("Drop your files, or click to choose");
    await click("Pronunciation corrections");
    const panel = document.querySelector('[aria-label="Pronunciation correction memory"]');
    check(panel, "Correction review must render after loading native memory");
    check(panel.textContent!.includes("Baseline: verse-lingua-lexical-context-v1."), "Correction memory must show the deterministic baseline without optional resources");
    check(!/\blaya\b|unavailable|qualified local.*evidence/i.test(panel.textContent!), "Correction memory must not expose rejected model availability");
    const referenceSelector = panel.querySelector<HTMLSelectElement>('select[aria-label="Original export reference"]');
    check(referenceSelector, "Named original references must be selectable");
    const named = [...referenceSelector.options].find((option) => option.textContent?.includes("Choir original.mscz") && option.textContent.includes("Choir reviewed export.ustx"));
    check(named && named.value === "export-reference", "Source and export names must identify the correct immutable reference");
    const browseLabel = "Drop an edited .ustx file here, or click to browse";
    check(!button(browseLabel).disabled && button("Compare corrected USTX copy").disabled, "An edited project must be loadable before choosing its original reference");
    const comparisonsBeforeLoad = calls.filter((call) => call.command === "pronunciation_compare").length;
    await click(browseLabel);
    check(panel.textContent!.includes("Selected project: /test/choir-edited.ustx"), "Browsing must visibly select the edited USTX file");
    check(button("Compare corrected USTX copy").disabled && calls.filter((call) => call.command === "pronunciation_compare").length === comparisonsBeforeLoad,
      "File selection must neither compare without a reference nor write correction memory");
    check(!calls.some((call) => call.command === "pronunciation_confirm"), "Loading a file must not save corrections");
    record("Edited USTX browse works before reference selection without comparison or memory writes");
    correctedDialogPath = null;
    await click(browseLabel);
    check(panel.textContent!.includes("/test/choir-edited.ustx"), "Cancelling browse must preserve the selected project");
    record("Cancelling edited-project browse preserves selection");
    await act(async () => { referenceSelector.value = named.value; referenceSelector.dispatchEvent(new Event("change", { bubbles: true })); });
    check(referenceSelector.value === "export-reference" && !button("Compare corrected USTX copy").disabled, "Choosing a named reference must enable comparison for that exact ID");
    record("Human-readable source/export labels select the correct immutable reference");
    await click("Compare corrected USTX copy");
    check(last("pronunciation_compare").referenceId === "export-reference" && last("pronunciation_compare").correctedPath === "/test/choir-edited.ustx",
      "Comparison must use the selected reference and previously loaded file");
    const analysesBeforeDrop = calls.filter((call) => call.command === "pronunciation_convert_files").length;
    await drop(["/test/replaced.USTX"]);
    check(panel.textContent!.includes("Selected project: /test/replaced.USTX") && !panel.textContent!.includes("pronunciation changes available for review"),
      "Native drop must replace the file and clear its old comparison");
    check(calls.filter((call) => call.command === "pronunciation_convert_files").length === analysesBeforeDrop, "Correction-screen drops must never reach source conversion");
    record("Native USTX drop selects a file and clears stale review without source conversion");
    for (const paths of [["/test/source.mscz"], ["/test/one.ustx", "/test/two.ustx"], []]) {
      await drop(paths);
      check(panel.querySelector('[role="alert"]')?.textContent?.includes("Choose one edited OpenUtau .ustx project"), "Invalid drops must explain the required input");
      check(panel.textContent!.includes("Selected project: /test/replaced.USTX"), "Invalid drops must preserve the last valid file");
      check(calls.filter((call) => call.command === "pronunciation_convert_files").length === analysesBeforeDrop, "Invalid correction-screen drops must never trigger score conversion");
    }
    record("Invalid and multiple drops explain the error and preserve the selected USTX");
    comparisonMode = "pending";
    await click("Compare corrected USTX copy");
    check(button(browseLabel).disabled && referenceSelector.disabled && finishComparison, "Pending comparison must disable both inputs");
    check(button("Pronunciation corrections").disabled && header().disabled, "Header must not close or change the profile during correction operations");
    await drop(["/test/ignored.ustx"]);
    check(panel.textContent!.includes("Selected project: /test/replaced.USTX"), "Busy comparison must ignore replacement drops");
    comparisonMode = "accept";
    await act(async () => { finishComparison!(); });
    check(!button(browseLabel).disabled, "Comparison completion must restore file loading");
    await act(async () => { referenceSelector.value = "other-reference"; referenceSelector.dispatchEvent(new Event("change", { bubbles: true })); });
    check(!panel.textContent!.includes("pronunciation changes available for review"), "Changing the original reference must clear stale comparison");
    comparisonMode = "reject";
    await click("Compare corrected USTX copy");
    check(panel.querySelector('[role="alert"]')?.textContent?.includes("Injected comparison rejection") && !button(browseLabel).disabled,
      "Failed comparisons must display the error and allow another file");
    comparisonMode = "accept";
    record("Busy and failed comparisons preserve inputs and reference changes clear stale review");
    const selectReviewedCorrection = async () => {
      const inputs = [...panel.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
      const proposal = inputs.find((input) => input.closest("label")?.textContent?.includes("ciel"));
      const listening = inputs.find((input) => input.closest("label")?.textContent?.includes("I listened with the assigned voices"));
      check(proposal && listening && !proposal.checked && !listening.checked && button("Save confirmed corrections").disabled,
        "A new comparison must require fresh selection and listening confirmation");
      await act(async () => { proposal.click(); listening.click(); });
      check(!button("Save confirmed corrections").disabled, "Explicit selection and listening must enable saving");
    };
    await click("Compare corrected USTX copy");
    await selectReviewedCorrection();
    await drop(["/test/final-edited.ustx"]);
    await click("Compare corrected USTX copy");
    await selectReviewedCorrection();
    await act(async () => { referenceSelector.value = "export-reference"; referenceSelector.dispatchEvent(new Event("change", { bubbles: true })); });
    await click("Compare corrected USTX copy");
    await selectReviewedCorrection();
    await click("Save confirmed corrections");
    const confirmation = last("pronunciation_confirm");
    check(confirmation.referenceId === "export-reference" && confirmation.correctedPath === "/test/final-edited.ustx" &&
      JSON.stringify(confirmation.selections) === JSON.stringify([{ correction_id: "reviewed-correction", scope: "occurrence_only", listened: true }]),
      "Confirmation must bind only freshly selected/listened corrections to the current inputs");
    record("Replacing file or reference resets selection and listening before exact confirmation");
    check(panel.textContent!.includes("en/s en/iy en/l") && panel.textContent!.includes("fr/s fr/y fr/ae fr/l"), "Before/after phone readings must be visible");
    check(panel.textContent!.includes("le [ciel] bleu") && panel.textContent!.includes("occurrence 3"), "Context and original occurrence must be reviewable");
    record("Imported correction exposes readings, phones, context and ownership");
    check(panel.textContent!.includes("Reviewed singer") && panel.textContent!.includes("duration-and-acoustic-inventory") && panel.textContent!.includes("voice-configuration"), "Singer constraints must be visible before confirmation");
    check(panel.textContent!.includes("Observed singer: Observed singer only") && panel.textContent!.includes("This identity does not qualify inventory or reuse"), "Observed singer identity must stay distinct from qualified voice constraints");
    check(panel.textContent!.includes("Unqualified phones remain excluded"), "Unknown phone validation cannot be promoted to automatic reuse");
    record("Imported correction exposes voice constraints and unknown symbol qualification");
    check(button("Activate reviewed correction").disabled, "Pending import requires explicit listening confirmation");
    check(!calls.some((c) => c.command === "pronunciation_set_status"), "Viewing/importing a record never activates it");
    record("Pending correction remains inactive without an explicit listening check");
    const listening = [...panel.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')].find((input) => input.closest("label")?.textContent?.includes("I reviewed this word"));
    check(listening, "Imported records need their own listening confirmation");
    await act(async () => { listening.click(); });
    await click("Activate reviewed correction");
    check(last("pronunciation_set_status").confirmed === true && last("pronunciation_set_status").id === "imported-correction", "Only checked reviewed record can activate");
    const previousAnalyses = calls.filter((c) => c.command === "pronunciation_convert_files").length;
    await click("Reanalyse loaded songs");
    check(calls.filter((c) => c.command === "pronunciation_convert_files").length === previousAnalyses + 1, "Reviewed memory must be reanalysable with loaded source paths");
    check(calls.some((c) => c.command === "pronunciation_release_snapshots"), "Reanalysis must release retired snapshots to recover quota");
    record("Explicit imported-record listening confirmation reaches native activation");
    await click("Close");
    const analysesAfterClose = calls.filter((call) => call.command === "pronunciation_convert_files").length;
    await drop(["/test/song.mscz"]);
    check(calls.filter((call) => call.command === "pronunciation_convert_files").length === analysesAfterClose + 1, "Closing corrections must restore exactly one source-drop handler");
    await act(async () => { root.render(null); });
    delayListenerCompletion = true;
    await mount();
    check(pendingListenerCompletions.length > 0, "Delayed listener regression must leave native registration unresolved");
    delayListenerCompletion = false;
    await click("Pronunciation corrections");
    const analysesBeforeDelayedDrop = calls.filter((call) => call.command === "pronunciation_convert_files").length;
    await drop(["/test/source.mscz"]);
    check(calls.filter((call) => call.command === "pronunciation_convert_files").length === analysesBeforeDelayedDrop,
      "Retired App callbacks must ignore drops even before their native registration completes");
    await act(async () => { for (const resolve of pendingListenerCompletions.splice(0)) resolve(); });
    record("Source drops resume once after close and retired delayed listeners cannot convert");
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
