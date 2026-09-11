import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { access, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";
import { cleanupBrowser, launchBrowserProcess, waitForDevTools, withDeadline } from "../scripts/browser-lifecycle.mjs";

const fixture = `
  const fs = require('node:fs');
  const path = require('node:path');
  const [profile, mode] = process.argv.slice(1);
  const activePort = path.join(profile, 'DevToolsActivePort');
  if (mode === 'exit') {
    process.stderr.write('x'.repeat(20000) + 'intentional startup failure', () => process.exit(17));
  } else {
    setInterval(() => {}, 1000);
    process.on('SIGTERM', () => {
      if (mode === 'ignore') return;
      fs.writeFileSync(path.join(profile, 'stopping'), 'still writing');
      setTimeout(() => {
        fs.writeFileSync(path.join(profile, 'last-write'), 'finished');
        process.exit(0);
      }, 200);
    });
    fs.writeFileSync(path.join(profile, 'ready'), 'ready');
    if (mode !== 'never') {
      setTimeout(() => {
        fs.writeFileSync(activePort, '12345');
        setTimeout(() => fs.appendFileSync(activePort, '\\n/devtools/browser/test\\n'), 100);
      }, mode === 'slow' ? 350 : 0);
    }
  }
`;

async function start(t, mode) {
  const profile = await mkdtemp(join(tmpdir(), "verse-lifecycle-test-"));
  const browser = launchBrowserProcess(process.execPath, ["-e", fixture, profile, mode]);
  t.after(() => cleanupBrowser(browser, profile, { stopTimeoutMs: 1_000, log() {} }));
  return { browser, profile };
}

async function waitForMarker(path) {
  return withDeadline(async (signal) => {
    while (true) {
      try { return await readFile(path, "utf8"); }
      catch (error) { if (error.code !== "ENOENT") throw error; }
      await delay(10, undefined, { signal });
    }
  }, 3_000, "fixture readiness");
}

const endpoint = (browser, profile, timeoutMs) => withDeadline(
  (signal) => browser.guard(waitForDevTools(browser, profile, signal)), timeoutMs, "Chromium startup",
);

test("slow startup and a partial port file succeed within the configured deadline", async (t) => {
  const { browser, profile } = await start(t, "slow");
  await assert.rejects(endpoint(browser, profile, 100), /timed out/);
  assert.equal(await endpoint(browser, profile, 2_000), "ws://127.0.0.1:12345/devtools/browser/test");
});

test("missing DevTools endpoint fails at the bounded startup deadline", async (t) => {
  const { browser, profile } = await start(t, "never");
  await waitForMarker(join(profile, "ready"));
  await assert.rejects(endpoint(browser, profile, 100), /Chromium startup timed out after 100 ms/);
});

test("browser exit beats the startup deadline with bounded stderr", async (t) => {
  const { browser, profile } = await start(t, "exit");
  await assert.rejects(endpoint(browser, profile, 10_000), /Chromium exited \(code 17/);
  assert.deepEqual(await browser.completion, { code: 17, signal: null });
  assert.match(browser.diagnostics(), /intentional startup failure/);
  assert.ok(Buffer.byteLength(browser.diagnostics()) < 16 * 1024 + 100);
});

test("spawn failure is observed immediately without an unhandled process error", async () => {
  const browser = launchBrowserProcess(join(tmpdir(), "verse-nonexistent-browser", "chrome"), []);
  await assert.rejects(withDeadline(() => browser.guard(new Promise(() => {})), 10_000, "startup"), /spawn\/process failure:.*ENOENT/);
  await browser.stop();
});

test("original kill-then-delete lifecycle races a deterministic final profile write", async (t) => {
  const { browser, profile } = await start(t, "normal");
  await endpoint(browser, profile, 2_000);
  browser.child.kill("SIGTERM");
  await waitForMarker(join(profile, "stopping"));
  // Reproduce the old harness's immediate deletion while Chromium still runs.
  assert.equal(browser.child.exitCode, null);
  await rm(profile, { recursive: true, force: true });
  assert.equal((await browser.completion).code, 1);
  assert.match(browser.diagnostics(), /ENOENT.*last-write/s);
});

test("cleanup awaits delayed shutdown and its final write before deleting the profile", async (t) => {
  const { browser, profile } = await start(t, "normal");
  await waitForMarker(join(profile, "ready"));
  const cleaning = cleanupBrowser(browser, profile, { stopTimeoutMs: 2_000 });
  assert.equal(await waitForMarker(join(profile, "stopping")), "still writing");
  await cleaning;
  assert.deepEqual(await browser.completion, { code: 0, signal: null });
  assert.equal(browser.diagnostics(), "\nChromium stderr: (empty)");
  await assert.rejects(access(profile), { code: "ENOENT" });
});

test("SIGTERM-resistant owned child is force-stopped and reaped before deletion", async (t) => {
  const { browser, profile } = await start(t, "ignore");
  await waitForMarker(join(profile, "ready"));
  const logs = [];
  await cleanupBrowser(browser, profile, { stopTimeoutMs: 100, log: (message) => logs.push(message) });
  assert.deepEqual(await browser.completion, { code: null, signal: "SIGKILL" });
  assert.match(logs.join("\n"), /graceful shutdown timed out.*Force-stopping owned Chromium child/s);
  await assert.rejects(access(profile), { code: "ENOENT" });
});

test("failed stop preserves the profile and surfaces cleanup failure", async (t) => {
  const profile = await mkdtemp(join(tmpdir(), "verse-lifecycle-test-"));
  t.after(() => rm(profile, { recursive: true, force: true }));
  await assert.rejects(cleanupBrowser({ stop: async () => { throw new Error("cannot stop owned child"); } }, profile), /cannot stop/);
  await access(profile);
});

test("successful deadline releases its 60-second timer and process exits promptly", async () => {
  const helper = new URL("../scripts/browser-lifecycle.mjs", import.meta.url).href;
  const browser = launchBrowserProcess(process.execPath, ["--input-type=module", "-e", `
    import { withDeadline } from ${JSON.stringify(helper)};
    await withDeadline(() => Promise.resolve('receipt'), 60000, 'receipt');
  `]);
  try {
    assert.deepEqual(await withDeadline(() => browser.completion, 3_000, "successful process exit"), { code: 0, signal: null });
  } finally { await browser.stop(); }
});

test("real harness spawn failure exits nonzero after cleanup with useful diagnostics", async () => {
  const child = spawn(process.execPath, ["scripts/test-pronunciation-browser.mjs"], {
    env: { ...process.env, VERSE_TEST_CHROME: join(tmpdir(), "verse-nonexistent-browser", "chrome") },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stderr.on("data", (chunk) => { output += chunk; });
  const closed = new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code) => resolve(code));
  });
  try {
    assert.equal(await withDeadline(() => closed, 10_000, "intentional harness failure"), 1);
    assert.match(output, /Chromium spawn\/process failure:.*ENOENT/);
    assert.doesNotMatch(output, /Unhandled|Browser cleanup failed/);
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await closed;
  }
});

test("real browser assertion failure keeps a nonzero result and removes owned resources", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "verse-intentional-browser-failure-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, "tests"));
  await mkdir(join(root, "profiles"));
  // Isolated failing page; the production App test page and its assertions stay intact.
  await writeFile(join(root, "tests/pronunciation-browser.html"), `<!doctype html>
    <button id="run-tests">Run</button>
    <script>
      document.getElementById('run-tests').onclick = async () => {
        try { if (1 !== 2) throw new Error('Intentional rendered assertion failure'); }
        catch (error) {
          await fetch('/__pronunciation-results', { method: 'POST', body: JSON.stringify({
            passed: false, tests: [], error: error.message
          }) });
        }
      };
    </script>`);
  const report = join(root, "receipt.json");
  const harness = fileURLToPath(new URL("../scripts/test-pronunciation-browser.mjs", import.meta.url));
  const child = spawn(process.execPath, [harness], {
    cwd: root,
    env: { ...process.env, VERSE_BROWSER_REPORT: report, TMPDIR: join(root, "profiles") },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stderr.on("data", (chunk) => { output += chunk; });
  const closed = new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code) => resolve(code));
  });
  try {
    assert.equal(await withDeadline(() => closed, 45_000, "intentional rendered failure"), 1);
    assert.match(output, /Intentional rendered assertion failure/);
    assert.equal(JSON.parse(await readFile(report, "utf8")).passed, false);
    assert.deepEqual((await readdir(join(root, "profiles"))).filter((name) => name.startsWith("verse-pronunciation-browser-")), []);
    assert.doesNotMatch(output, /Unhandled|Browser cleanup failed/);
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await closed;
  }
});
