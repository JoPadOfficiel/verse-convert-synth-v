import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

async function load(path, dependencies = {}) {
  const source = await readFile(new URL(path, import.meta.url), "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  const exports = {};
  new Function("require", "exports", compiled)((name) => {
    assert.ok(name in dependencies, `Unexpected dependency ${name}`);
    return dependencies[name];
  }, exports);
  return exports;
}

const calls = [];
let invokeResult = [];
const api = await load("../src/lib/tauri.ts", {
  "@tauri-apps/api/core": {
    Channel: class {},
    invoke: async (command, payload) => { calls.push({ command, payload }); return invokeResult; },
  },
  "@tauri-apps/plugin-dialog": { save: async () => "/tmp/song.ustx" },
  "@/lib/file-utils": await load("../src/lib/file-utils.ts"),
});

const rustAdapter = await readFile(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const registrations = [...rustAdapter.matchAll(/tauri::generate_handler!\[([\s\S]*?)\]/g)];
assert.equal(registrations.length, 1, "Audit the actual application IPC registration");
const registeredCommands = registrations[0][1].split(",").map((name) => name.trim()).filter(Boolean);

test("registered conversion and export RPCs expose only frozen-snapshot adapters", () => {
  assert.deepEqual(registeredCommands.toSorted(), [
    "renderer_status",
    "pronunciation_convert_files", "pronunciation_export_svp", "pronunciation_export_bundle",
    "pronunciation_memory", "pronunciation_compare", "pronunciation_confirm",
    "pronunciation_set_status", "pronunciation_exchange", "pronunciation_cancel",
    "pronunciation_release_snapshots",
  ].toSorted());
  for (const legacy of ["convert_files", "export_svp", "export_bundle"]) {
    assert.ok(!registeredCommands.includes(legacy), `${legacy} must not bypass snapshot enforcement`);
  }
});

test("frontend direct, batch and bundle exports use registered RPCs with the analysed snapshot", async () => {
  const file = { path: "/tmp/frozen-snapshot.mscz", ok: true, pronunciationSnapshotId: "analysis:sealed" };
  calls.length = 0;
  try {
    invokeResult = [file];
    await api.convertFiles([file.path], false);
    invokeResult = [];
    await api.exportVocalsWithDialog(file, "english");
    await api.exportBundle(file, "/tmp/frozen-snapshot.versebundle", "english");
    await api.convertFiles([file.path], true);
    assert.deepEqual(calls.map(({ command }) => command), [
      "pronunciation_convert_files", "pronunciation_export_svp",
      "pronunciation_export_bundle", "pronunciation_convert_files",
    ]);
    for (const { command } of calls) assert.ok(registeredCommands.includes(command), `${command} is registered`);
    assert.equal(calls[0].payload.write, false);
    assert.equal(calls[3].payload.write, true);
    assert.equal(calls[0].payload.snapshotIds, null);
    assert.equal(calls[1].payload.snapshotId, file.pronunciationSnapshotId);
    assert.equal(calls[2].payload.snapshotId, file.pronunciationSnapshotId);
    assert.deepEqual(calls[3].payload.snapshotIds, { [file.path]: file.pronunciationSnapshotId });
  } finally {
    invokeResult = [];
  }
});

test("analysis, direct and bundle adapters carry one explicit pronunciation selection", async () => {
  for (const profile of ["default", "automatic", "frenchMillefeuille", "englishArpabet", "spanishDiffSinger", "portugueseDiffSinger"]) {
    calls.length = 0;
    await api.convertFiles(["/tmp/song.mscz"], false, "english", undefined, undefined, "ustx", profile);
    await api.exportVocalsWithDialog({ path: "/tmp/song.mscz" }, "english", undefined, "ustx", profile);
    await api.exportBundle({ path: "/tmp/song.mscz" }, "/tmp/song.versebundle", "english", undefined, undefined, undefined, "ustx", profile);
    assert.deepEqual(calls.map(({ command }) => command), ["pronunciation_convert_files", "pronunciation_export_svp", "pronunciation_export_bundle"]);
    for (const { payload } of calls) {
      assert.equal(payload.pronunciationProfile, profile);
      assert.equal(payload.exportTarget, "ustx");
      assert.equal(payload.language, "english", "legacy language stays independent");
    }
  }
});

test("omitting pronunciation selects Automatic consistently in every adapter", async () => {
  calls.length = 0;
  await api.convertFiles(["/tmp/song.mscz"], false);
  await api.exportVocalsWithDialog({ path: "/tmp/song.mscz" }, "english");
  await api.exportBundle({ path: "/tmp/song.mscz" }, "/tmp/song.versebundle", "english");
  assert.equal(calls.length, 3);
  for (const call of calls) assert.equal(call.payload.pronunciationProfile, "automatic");
  assert.equal(calls[0].payload.exportTarget, "svp");
});

// Execute the actual App callback without installing a DOM or React test
// framework. TypeScript's existing parser locates it; setters model observable
// state updates while the real async control flow and verdict handling run.
const appSource = ts.createSourceFile("App.tsx", await readFile(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let changeTargetSource;
function findChangeTarget(node) {
  if (ts.isFunctionDeclaration(node) && node.name?.text === "changeTarget") changeTargetSource = node.getText(appSource);
  ts.forEachChild(node, findChangeTarget);
}
findChangeTarget(appSource);
assert.ok(changeTargetSource, "App must expose its actual reanalysis callback");
const callbackCode = ts.transpileModule(changeTargetSource, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;

function reanalysisHarness(convertFiles, initialProfile = "default") {
  const state = { items: [{ path: "song.mscz", ok: true }], exportTarget: "ustx", pronunciationProfile: initialProfile,
    selected: new Set(["song.mscz"]), exportErrors: { "song.mscz": "old export error" }, exportProgress: { "song.mscz": {} }, globalError: null, busy: false, savedProfiles: [] };
  const scope = { items: state.items, exportTarget: state.exportTarget, pronunciationProfile: state.pronunciationProfile,
    language: "english", overrides: {}, convertFiles, commandErrorMessage: (error) => error.message,
    storePronunciationProfile: (profile) => state.savedProfiles.push(profile),
    beginBusy: () => { if (state.busy) return false; state.busy = true; return true; }, endBusy: () => { state.busy = false; } };
  for (const key of ["items", "exportTarget", "pronunciationProfile", "selected", "exportErrors", "exportProgress", "globalError"]) {
    scope[`set${key[0].toUpperCase()}${key.slice(1)}`] = (value) => { state[key] = typeof value === "function" ? value(state[key]) : value; };
  }
  return { state, change: new Function(...Object.keys(scope), `${callbackCode}\nreturn changeTarget;`)(...Object.values(scope)) };
}

for (const [profile, unsupported] of [
  ["automatic", "AUTOMATIC_LANGUAGE_LOW_CONFIDENCE"],
  ["frenchMillefeuille", "FRENCH_PRONUNCIATION_UNSUPPORTED"],
  ["englishArpabet", "ENGLISH_PRONUNCIATION_UNSUPPORTED"],
  ["spanishDiffSinger", "DIFFSINGER_PRONUNCIATION_INCOMPLETE_WORD"],
  ["portugueseDiffSinger", "DIFFSINGER_PRONUNCIATION_INCOMPLETE_WORD"],
]) {
test(`${profile}: same-target pronunciation change waits for reanalysis and adopts returned per-file verdicts`, async () => {
  let resolve;
  const requested = [];
  const { state, change } = reanalysisHarness((...args) => { requested.push(args); return new Promise((done) => { resolve = done; }); });
  const pending = change("ustx", profile);
  assert.equal(requested.length, 1);
  assert.deepEqual(requested[0], [["song.mscz"], false, "english", undefined, {}, "ustx", profile]);
  assert.equal(state.busy, true);
  assert.equal(state.pronunciationProfile, "default", "no selection change before the command returns");
  assert.deepEqual(state.savedProfiles, [], "pending analysis must not change the next session's profile");
  const verdicts = [{ path: "song.mscz", ok: false, msg: "target cannot represent this source" },
    { path: "other.mscz", ok: true, warnings: [{ code: unsupported }] }];
  resolve(verdicts);
  await pending;
  assert.equal(state.pronunciationProfile, profile);
  assert.deepEqual(state.savedProfiles, [profile]);
  assert.equal(state.items, verdicts, "not-ok and unsupported diagnostics are valid new-profile verdicts");
  assert.equal(state.selected.size, 0);
  assert.deepEqual(state.exportErrors, {});
  assert.deepEqual(state.exportProgress, {});
  assert.equal(state.globalError, null);
  assert.equal(state.busy, false);
});

test(`${profile}: rejected reanalysis preserves the old selection and diagnostics`, async () => {
  const { state, change } = reanalysisHarness(async () => { throw new Error("command unavailable"); });
  const original = state.items;
  await change("ustx", profile);
  assert.equal(state.items, original);
  assert.equal(state.pronunciationProfile, "default");
  assert.deepEqual(state.savedProfiles, [], "failed analysis must not persist an unaccepted choice");
  assert.equal(state.globalError, "command unavailable");
  assert.equal(state.exportErrors["song.mscz"], "old export error");
  assert.equal(state.busy, false);
});

test(`${profile}: unchanged selection or the active busy guard prevents another reanalysis`, async () => {
  const { state, change } = reanalysisHarness(async () => { assert.fail("unexpected reanalysis"); });
  await change("ustx", "default");
  state.busy = true;
  await change("ustx", profile);
  assert.equal(state.pronunciationProfile, "default");
  assert.equal(state.busy, true, "another operation still owns the guard");
  assert.deepEqual(state.savedProfiles, []);
});
}

test("choosing a profile before import is remembered without running an analysis", async () => {
  for (const profile of ["automatic", "frenchMillefeuille", "englishArpabet", "spanishDiffSinger", "portugueseDiffSinger", "default"]) {
    const previous = profile === "default" ? "frenchMillefeuille" : "default";
    const { state, change } = reanalysisHarness(async () => assert.fail("no files to analyse"), previous);
    state.items.length = 0;
    state.busy = true;
    await change("ustx", profile);
    assert.equal(state.pronunciationProfile, previous, "an initial import may own the guard while the list is empty");
    assert.deepEqual(state.savedProfiles, []);
    state.busy = false;
    await change("ustx", profile);
    assert.equal(state.pronunciationProfile, profile);
    assert.deepEqual(state.savedProfiles, [profile]);
    assert.equal(state.busy, false);
  }
});

test("returning to Default is persisted only after accepted loaded-score reanalysis", async () => {
  for (const reject of [false, true]) {
    const { state, change } = reanalysisHarness(async () => {
      if (reject) throw new Error("rejected Default analysis");
      return [{ path: "song.mscz", ok: true }];
    }, "frenchMillefeuille");
    await change("ustx", "default");
    assert.equal(state.pronunciationProfile, reject ? "frenchMillefeuille" : "default");
    assert.deepEqual(state.savedProfiles, reject ? [] : ["default"]);
  }
});

test("pronunciation preference survives restart, rejects stale values and tolerates disabled storage", async () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  try {
    const values = new Map();
    Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
    } });
    const preference = await load("../src/lib/pronunciation-preference.ts");
    assert.equal(preference.storedPronunciationProfile(), "automatic");
    for (const profile of ["automatic", "frenchMillefeuille", "englishArpabet", "spanishDiffSinger", "portugueseDiffSinger", "default"]) {
      preference.storePronunciationProfile(profile);
      const restarted = await load("../src/lib/pronunciation-preference.ts");
      assert.equal(restarted.storedPronunciationProfile(), profile);
    }
    values.set("verse.pronunciationProfile", "automaticFrenchEnglish");
    assert.equal(preference.storedPronunciationProfile(), "automatic");
    preference.storePronunciationProfile(preference.storedPronunciationProfile());
    assert.equal(values.get("verse.pronunciationProfile"), "automatic");
    for (const value of ["unknown", "French", "", "null"]) {
      values.set("verse.pronunciationProfile", value);
      assert.equal(preference.storedPronunciationProfile(), "automatic");
    }
    Object.defineProperty(globalThis, "localStorage", { configurable: true, get() { throw new Error("storage disabled"); } });
    assert.equal(preference.storedPronunciationProfile(), "automatic");
    assert.doesNotThrow(() => preference.storePronunciationProfile("frenchMillefeuille"));
  } finally {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  }
});


test("memory reanalysis callback replaces FileResults and their snapshots", async () => {
  let source;
  function find(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === "reanalyseLoaded") source = node.getText(appSource);
    ts.forEachChild(node, find);
  }
  find(appSource);assert.ok(source);
  const result=[{path:"song.mscz",ok:true,pronunciationSnapshotId:"new-analysis"}];
  let items=[{path:"song.mscz",ok:true,pronunciationSnapshotId:"retired-analysis"}];let ended=false;
  const callback=new Function("items","beginBusy","endBusy","setGlobalError","convertFiles","language","overrides","exportTarget","pronunciationProfile","setItems","setExportErrors","setExportProgress","setSelected","commandErrorMessage",
    ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText+"\nreturn reanalyseLoaded;")(
      items,()=>true,()=>{ended=true;},()=>{},async(paths,write)=>{assert.deepEqual(paths,["song.mscz"]);assert.equal(write,false);return result;},"english",{},"ustx","automatic",(value)=>{items=value;},()=>{},()=>{},()=>{},(e)=>e.message);
  await callback();assert.equal(items,result);assert.equal(items[0].pronunciationSnapshotId,"new-analysis");assert.ok(ended);
});
