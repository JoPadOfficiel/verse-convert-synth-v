import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { access, appendFile, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import { promisify } from "node:util";
import test from "node:test";
import { cleanupBrowser, launchBrowserProcess, ownedGroupAlive, waitForDevTools, withDeadline } from "../scripts/browser-lifecycle.mjs";

const posix = { skip: process.platform === "win32" ? "Requires POSIX signal semantics" : false };
const browserId = "01234567-89ab-cdef-0123-456789abcdef";

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
      if (mode === 'bad-stop') process.exit(23);
      fs.writeFileSync(path.join(profile, 'stopping'), 'still writing');
      setTimeout(() => {
        fs.writeFileSync(path.join(profile, 'last-write'), 'finished');
        process.exit(0);
      }, 200);
    });
    fs.writeFileSync(path.join(profile, 'ready'), 'ready');
    if (mode !== 'never' && mode !== 'split') {
      setTimeout(() => {
        fs.writeFileSync(activePort, '12345');
        setTimeout(() => fs.appendFileSync(activePort, '\\n/devtools/browser/${browserId}\\n'), 100);
      }, mode === 'slow' ? 350 : 0);
    }
  }
`;

async function start(t, mode) {
  const profile = await mkdtemp(join(tmpdir(), "verse-lifecycle-test-"));
  const browser = launchBrowserProcess(process.execPath, ["-e", fixture, profile, mode]);
  t.after(async () => {
    try { await browser.stop({ stopTimeoutMs: 1_000, log() {} }); }
    catch (error) { if (!browser.stopped) throw error; }
    finally { if (browser.stopped) await rm(profile, { recursive: true, force: true }); }
  });
  return { browser, profile };
}

async function waitForMarker(path) {
  return withDeadline(async (signal) => {
    while (true) {
      try {
        const value = await readFile(path, "utf8");
        if (value) return value;
      }
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
  assert.equal(await endpoint(browser, profile, 2_000), `ws://127.0.0.1:12345/devtools/browser/${browserId}`);
});

test("split browser UUID waits for the complete identifier", async (t) => {
  const { browser, profile } = await start(t, "split");
  await waitForMarker(join(profile, "ready"));
  await writeFile(join(profile, "DevToolsActivePort"), `12345\n/devtools/browser/${browserId.slice(0, 20)}`);
  await assert.rejects(endpoint(browser, profile, 150), /timed out/);
  await appendFile(join(profile, "DevToolsActivePort"), browserId.slice(20) + "\n");
  assert.equal(await endpoint(browser, profile, 2_000), `ws://127.0.0.1:12345/devtools/browser/${browserId}`);
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

test("original kill-then-delete lifecycle races a deterministic final profile write", posix, async (t) => {
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

test("cleanup awaits delayed shutdown and its final write before deleting the profile", posix, async (t) => {
  const { browser, profile } = await start(t, "normal");
  await waitForMarker(join(profile, "ready"));
  const cleaning = cleanupBrowser(browser, profile, { stopTimeoutMs: 2_000 });
  assert.equal(await waitForMarker(join(profile, "stopping")), "still writing");
  await cleaning;
  assert.deepEqual(await browser.completion, { code: 0, signal: null });
  assert.equal(browser.diagnostics(), "\nChromium stderr: (empty)");
  await assert.rejects(access(profile), { code: "ENOENT" });
});

test("SIGTERM-resistant owned child is force-stopped and reaped before deletion", posix, async (t) => {
  const { browser, profile } = await start(t, "ignore");
  await waitForMarker(join(profile, "ready"));
  const logs = [];
  await cleanupBrowser(browser, profile, { stopTimeoutMs: 100, log: (message) => logs.push(message) });
  assert.deepEqual(await browser.completion, { code: null, signal: "SIGKILL" });
  assert.match(logs.join("\n"), /graceful shutdown timed out.*Force-stopping owned Chromium child/s);
  await assert.rejects(access(profile), { code: "ENOENT" });
});

test("nonzero exit during requested shutdown is surfaced", posix, async (t) => {
  const { browser, profile } = await start(t, "bad-stop");
  await waitForMarker(join(profile, "ready"));
  await assert.rejects(cleanupBrowser(browser, profile), /exited unexpectedly during shutdown \(code 23/);
  assert.equal(browser.stopped, true);
  await access(profile);
});

test("owned POSIX descendant is reaped before cleanup completes", posix, async (t) => {
  const profile = await mkdtemp(join(tmpdir(), "verse-descendant-test-"));
  const browser = launchBrowserProcess(process.execPath, ["-e", `
    const { spawn } = require('node:child_process');
    const fs = require('node:fs');
    const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: ['ignore', 'ignore', 2] });
    process.on('SIGTERM', () => { child.once('close', () => process.exit(0)); });
    child.on('spawn', () => fs.writeFileSync(${JSON.stringify(join(profile, "ready"))}, String(child.pid)));
  `]);
  t.after(() => cleanupBrowser(browser, profile));
  const pid = Number(await waitForMarker(join(profile, "ready")));
  await cleanupBrowser(browser, profile, { stopTimeoutMs: 2_000 });
  assert.equal(browser.stopped, true);
  assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
  await assert.rejects(access(profile), { code: "ENOENT" });
});

test("Linux zombie-only owned group cleans up before its outside parent calls waitpid", {
  skip: process.platform !== "linux" ? "Requires Linux /proc and delayed waitpid" : false,
}, async (t) => {
  const root = await mkdtemp(join(tmpdir(), "verse-zombie-group-"));
  const profile = join(root, "profile");
  await mkdir(profile);
  const browser = launchBrowserProcess("python3", ["-c", `
import ctypes, json, os, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
group = os.getpgrp()
parent = os.fork()
if parent:
    deadline = time.monotonic() + 10
    while not (root / 'zombie.json').exists():
        if time.monotonic() >= deadline:
            os._exit(2)
        time.sleep(0.01)
    os._exit(0)

# Keep the reaper alive outside the owned browser group, without holding pipes.
os.setpgid(0, 0)
fd = os.open(os.devnull, os.O_RDWR)
for stream in (0, 1, 2):
    os.dup2(fd, stream)
if fd > 2:
    os.close(fd)
child = os.fork()
if child == 0:
    os.setpgid(0, group)
    # Exercise comm parsing with spaces, nested parentheses and a newline.
    ctypes.CDLL(None).prctl(15, b'verse ) (z)\\n', 0, 0, 0)
    os._exit(0)

observed = os.waitid(os.P_PID, child, os.WEXITED | os.WNOWAIT)
receipt = {'child': child, 'parent': os.getpid(), 'parentGroup': os.getpgrp(),
           'group': group, 'observed': observed.si_pid}
(root / 'zombie.tmp').write_text(json.dumps(receipt))
os.replace(root / 'zombie.tmp', root / 'zombie.json')
deadline = time.monotonic() + 20
while not (root / 'reap').exists() and time.monotonic() < deadline:
    time.sleep(0.01)
reaped, status = os.waitpid(child, 0)
(root / 'reaped').write_text(str(reaped))
os._exit(0)
`, root]);
  let receipt;
  t.after(async () => {
    await writeFile(join(root, "reap"), "reap now");
    receipt ??= JSON.parse(await waitForMarker(join(root, "zombie.json")));
    assert.equal(Number(await waitForMarker(join(root, "reaped"))), receipt.child);
    await stopOwnedGroup(receipt.parent);
    await browser.stop();
    await rm(root, { recursive: true, force: true });
  });
  receipt = JSON.parse(await waitForMarker(join(root, "zombie.json")));
  assert.equal(receipt.group, browser.child.pid);
  assert.notEqual(receipt.parentGroup, receipt.group);
  assert.equal(receipt.observed, receipt.child);
  assert.deepEqual(await withDeadline(() => browser.completion, 3_000, "group leader exit"), { code: 0, signal: null });
  const assertZombie = async () => {
    const stat = await readFile(`/proc/${receipt.child}/stat`, "utf8");
    assert.ok(stat.includes("(verse ) (z)\n)"));
    assert.match(stat.slice(stat.lastIndexOf(")") + 1), new RegExp(`^ Z ${receipt.parent} ${receipt.group} `));
    process.kill(-receipt.group, 0); // The old probe still reports an extant group.
  };
  await assertZombie();
  assert.equal(await ownedGroupAlive(receipt.group), false);
  const logs = [];
  await cleanupBrowser(browser, profile, { stopTimeoutMs: 1_000, log: (message) => logs.push(message) });
  assert.equal(browser.stopped, true);
  assert.deepEqual(logs, []);
  await assert.rejects(access(profile), { code: "ENOENT" });
  await withDeadline(() => stopOwnedGroup(receipt.group), 1_000, "zombie group supervisor cleanup");
  await assertZombie(); // Both cleanup paths finished while waitpid was withheld.
  await assert.rejects(access(join(root, "reaped")), { code: "ENOENT" });
});

test("descendant-held pipe timeout releases handles and preserves profile without killing escaped group", posix, async (t) => {
  const profile = await mkdtemp(join(tmpdir(), "verse-held-pipe-test-"));
  const helper = new URL("../scripts/browser-lifecycle.mjs", import.meta.url).href;
  const descendantPid = join(profile, "descendant.pid");
  const ready = join(profile, "ready");
  const childScript = `
    const { spawn } = require('node:child_process');
    const fs = require('node:fs');
    const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {
      detached: true, stdio: ['ignore', 'ignore', 2]
    });
    child.on('spawn', () => {
      fs.writeFileSync(${JSON.stringify(descendantPid)}, String(child.pid));
      fs.writeFileSync(${JSON.stringify(ready)}, 'ready');
    });
    setInterval(() => {}, 1000);
  `;
  const supervisor = launchBrowserProcess(process.execPath, ["--input-type=module", "-e", `
    import { readFile, access } from 'node:fs/promises';
    import { setTimeout as delay } from 'node:timers/promises';
    import { launchBrowserProcess, cleanupBrowser } from ${JSON.stringify(helper)};
    const browser = launchBrowserProcess(process.execPath, ['-e', ${JSON.stringify(childScript)}]);
    while (true) { try { await readFile(${JSON.stringify(ready)}); break; } catch { await delay(10); } }
    try { await cleanupBrowser(browser, ${JSON.stringify(profile)}, { stopTimeoutMs: 200 }); }
    catch (error) {
      console.error(error.message);
      await access(${JSON.stringify(profile)});
      process.exitCode = 1;
    }
  `]);
  let pid;
  t.after(async () => {
    pid ??= Number(await readFile(descendantPid, "utf8"));
    await stopOwnedGroup(pid);
    await supervisor.stop();
    await rm(profile, { recursive: true, force: true });
  });
  pid = Number(await waitForMarker(descendantPid));
  const result = await withDeadline(() => supervisor.completion, 5_000, "released supervisor exit");
  assert.equal(result.code, 1);
  assert.match(supervisor.diagnostics(), /Chromium forced shutdown timed out after 200 ms/);
  await access(profile);
  // The escaped group is outside the browser's owned group and remains untouched.
  process.kill(pid, 0);
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

// Only PIDs announced by our IPC-connected harness are eligible for cleanup.
async function stopOwnedGroup(pid) {
  assert.ok(Number.isInteger(pid) && pid > 0, "Owned process PID must be positive");
  const target = process.platform === "win32" ? pid : -pid;
  const alive = () => ownedGroupAlive(pid);
  if (!await alive()) return;
  const wait = (signal) => (async () => {
    while (await alive()) await delay(20, undefined, { signal });
  })();
  const signalOwned = (signal) => {
    try { process.kill(target, signal); }
    catch (error) { if (error.code !== "ESRCH") throw error; }
  };
  if (process.platform === "win32") {
    try { await promisify(execFile)("taskkill", ["/PID", String(pid), "/T", "/F"], { timeout: 3_000 }); }
    catch (error) { if (await alive()) throw error; }
  } else {
    signalOwned("SIGTERM");
    try { await withDeadline(wait, 2_000, "owned group graceful shutdown"); return; }
    catch (error) {
      if (!await alive()) return;
      signalOwned("SIGKILL");
    }
  }
  await withDeadline(wait, 3_000, "owned group forced shutdown");
}

async function runHarness(t, { page = "", env = {}, preload = "", outerTimeoutMs = 50_000 } = {}) {
  const root = await mkdtemp(join(tmpdir(), "verse-isolated-harness-"));
  const profiles = join(root, "profiles");
  const report = join(root, "receipt.json");
  let safeToDelete = false;
  t.after(async () => { if (safeToDelete) await rm(root, { recursive: true, force: true }); });
  await mkdir(join(root, "tests"));
  await mkdir(profiles);
  await writeFile(join(root, "tests/pronunciation-browser.html"), page);
  const args = [];
  if (preload) {
    const path = join(root, "preload.mjs");
    await writeFile(path, preload);
    args.push("--import", path);
  }
  args.push(fileURLToPath(new URL("../scripts/test-pronunciation-browser.mjs", import.meta.url)));
  const harness = launchBrowserProcess(process.execPath, args, {
    cwd: root, ipc: true,
    env: { ...process.env, VERSE_BROWSER_STARTUP_TIMEOUT_MS: "30000", VERSE_BROWSER_STOP_TIMEOUT_MS: "5000",
      ...env, VERSE_BROWSER_REPORT: report, TMPDIR: profiles, TMP: profiles, TEMP: profiles },
  });
  const owned = new Set();
  harness.child.on("message", (message) => {
    if (message.type === "verse-browser-owned" && Number.isInteger(message.pid) && message.pid > 0) owned.add(message.pid);
  });
  try {
    const result = await withDeadline(() => harness.completion, outerTimeoutMs, "outer harness deadline");
    const output = harness.diagnostics();
    assert.doesNotMatch(output, /Unhandled|Browser cleanup failed/);
    assert.deepEqual((await readdir(profiles)).filter((name) => name.startsWith("verse-pronunciation-browser-")), []);
    return { ...result, output, report };
  } finally {
    const errors = [];
    // Stop the nested browser group before its supervisor, including on timeout.
    for (const pid of owned) {
      try { await stopOwnedGroup(pid); } catch (error) { errors.push(error); }
    }
    try {
      await withDeadline(() => harness.completion, 15_000, "harness exit after browser cleanup").catch(() => {});
      await harness.stop({ stopTimeoutMs: 3_000 });
    } catch (error) { errors.push(error); }
    // Drain ownership announcements that raced the outer timeout before deletion.
    for (const pid of owned) {
      try { await stopOwnedGroup(pid); } catch (error) { errors.push(error); }
    }
    safeToDelete = errors.length === 0 && harness.stopped;
    if (errors.length) throw new AggregateError(errors,
      `Owned cleanup failed (${errors.map((error) => error.message).join("; ")}); preserved ${root}`);
  }
}

function receiptPage(result) {
  return `<!doctype html><button id="run-tests">Run</button><script>
    document.getElementById('run-tests').onclick = async () => {
      await fetch('/__pronunciation-results', { method: 'POST', body: JSON.stringify(${JSON.stringify(result)}) });
    };
  </script>`;
}

test("real harness spawn failure uses isolated report/profile paths", async (t) => {
  const result = await runHarness(t, {
    env: { VERSE_TEST_CHROME: join(tmpdir(), "verse-nonexistent-browser", "chrome") },
  });
  assert.equal(result.code, 1);
  assert.match(result.output, /Chromium spawn\/process failure:.*ENOENT/);
});

test("real browser assertion failure keeps a nonzero result and removes owned resources", async (t) => {
  const result = await runHarness(t, { page: `<!doctype html><button id="run-tests">Run</button><script>
    document.getElementById('run-tests').onclick = async () => {
      try { if (1 !== 2) throw new Error('Intentional rendered assertion failure'); }
      catch (error) {
        await fetch('/__pronunciation-results', { method: 'POST', body: JSON.stringify({
          passed: false, tests: [], error: error.message
        }) });
      }
    };
  </script>` });
  assert.equal(result.code, 1);
  assert.match(result.output, /Intentional rendered assertion failure/);
  assert.equal(JSON.parse(await readFile(result.report, "utf8")).passed, false);
});

for (const [name, tests, reloads] of [
  ["too few cases", Array.from({ length: 23 }, () => ({ passed: true })), 3],
  ["nonpassing entry", Array.from({ length: 24 }, (_, i) => ({ passed: i !== 3 })), 3],
  ["wrong reload count", Array.from({ length: 24 }, () => ({ passed: true })), 2],
]) {
  test(`incomplete successful receipt fails closed: ${name}`, async (t) => {
    const result = await runHarness(t, { page: receiptPage({ passed: true, tests, reloads }) });
    assert.equal(result.code, 1);
    assert.match(result.output, /Incomplete browser receipt: expected 24 passing cases and 3 reloads/);
  });
}

test("stalled DevTools handshake fails at startup deadline and cleans owned resources", async (t) => {
  const result = await runHarness(t, { preload: `
    import { createServer } from 'node:net';
    const server = createServer((socket) => socket.unref());
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    server.unref();
    const NativeWebSocket = globalThis.WebSocket;
    globalThis.WebSocket = class extends NativeWebSocket {
      constructor() { super('ws://127.0.0.1:' + server.address().port + '/stalled'); }
    };
  ` });
  assert.equal(result.code, 1);
  assert.match(result.output, /Chromium startup timed out after 30000 ms \(stage: DevTools connection\)/);
});

test("never-ready page fails at startup deadline and cleans owned resources", async (t) => {
  const result = await runHarness(t, { page: "<!doctype html><p>No run button</p>" });
  assert.equal(result.code, 1);
  assert.match(result.output, /Chromium startup timed out after 30000 ms \(stage: harness navigation\/readiness\)/);
});

test("outer harness deadline cleans its separately owned browser before removing fixture", async (t) => {
  await assert.rejects(runHarness(t, {
    page: "<!doctype html><p>No run button</p>",
    env: { VERSE_BROWSER_STARTUP_TIMEOUT_MS: "60000" }, outerTimeoutMs: 5_000,
  }), /outer harness deadline timed out/);
});
