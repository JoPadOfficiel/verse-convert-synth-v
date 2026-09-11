import { spawn } from "node:child_process";
import { readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

const stderrLimit = 16 * 1024;

export function timeoutSetting(name, fallback) {
  const value = Number(process.env[name] ?? fallback);
  if (!Number.isSafeInteger(value) || value <= 0 || value > 2_147_483_647) {
    throw new Error(`${name} must be an integer between 1 and 2147483647 ms`);
  }
  return value;
}

// Abort polling when a stage ends, and always release its timeout on success.
export async function withDeadline(operation, timeoutMs, label) {
  const controller = new AbortController();
  let timer;
  try {
    return await Promise.race([
      Promise.resolve().then(() => operation(controller.signal)),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} timed out after ${timeoutMs} ms`)), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
    controller.abort();
  }
}

export function launchBrowserProcess(command, args, { cwd, env, ipc = false } = {}) {
  const grouped = process.platform !== "win32";
  const child = spawn(command, args, {
    cwd, env, detached: grouped,
    stdio: ["ignore", "ignore", "pipe", ...(ipc ? ["ipc"] : [])],
  });
  let stderr = Buffer.alloc(0);
  let closed = false;
  let failure;
  let exit;
  let stopPromise;
  let stopped = false;
  let rejectFailure;
  const failed = new Promise((_, reject) => { rejectFailure = reject; });
  // Spawn/exit can precede the caller's first wait. Never reject unobserved.
  failed.catch(() => {});
  const diagnostics = () => stderr.length ? `\nChromium stderr (last ${stderrLimit} bytes):\n${stderr.toString("utf8")}` : "\nChromium stderr: (empty)";
  const fail = (message) => {
    failure ??= new Error(message);
    rejectFailure(failure);
  };
  child.stderr.on("data", (chunk) => {
    stderr = Buffer.concat([stderr, chunk]).subarray(-stderrLimit);
  });
  child.on("error", (error) => fail(`Chromium spawn/process failure: ${error.message}`));
  child.once("exit", (code, signal) => {
    exit = { code, signal };
    fail(`Chromium exited (code ${code}, signal ${signal ?? "none"})`);
  });
  const completion = new Promise((resolve) => {
    // 'close' follows exit/spawn failure and closure of the stderr pipe.
    child.once("close", (code, signal) => { closed = true; resolve({ code, signal }); });
  });
  const groupAlive = () => {
    if (!grouped || !child.pid) return false;
    try { process.kill(-child.pid, 0); return true; }
    catch (error) {
      if (error.code === "ESRCH") return false;
      if (error.code === "EPERM") return true;
      throw error;
    }
  };
  const signalOwned = (signal) => {
    if (!child.pid) return;
    try {
      if (grouped) process.kill(-child.pid, signal);
      else if (!exit) child.kill(signal);
    } catch (error) { if (error.code !== "ESRCH") throw error; }
  };
  const awaitStopped = async (signal) => {
    await completion;
    while (groupAlive()) await delay(20, undefined, { signal });
  };
  return {
    child,
    completion,
    diagnostics,
    get stopped() { return stopped; },
    guard: (operation) => Promise.race([operation, failed]),
    assertRunning() { if (failure) throw failure; },
    stop({ stopTimeoutMs = 5_000, log = console.error } = {}) {
      return stopPromise ??= (async () => {
        const requestedWhileRunning = !exit && !closed && Boolean(child.pid);
        let forced = false;
        try {
          signalOwned("SIGTERM");
          const graceful = await withDeadline(awaitStopped, stopTimeoutMs, "Chromium graceful shutdown")
            .then(() => true, (error) => { log(error.message); return false; });
          if (!graceful) {
            forced = true;
            log("Force-stopping owned Chromium child/process group with SIGKILL");
            signalOwned("SIGKILL");
            await withDeadline(awaitStopped, stopTimeoutMs, "Chromium forced shutdown");
          }
          stopped = true;
        } catch (error) {
          // A descendant may retain the pipe even after the direct child exits.
          // Release our handles, but never claim ownership stopped or delete its profile.
          child.stderr.destroy();
          child.unref();
          if (child.connected) child.disconnect();
          throw error;
        }
        if (requestedWhileRunning && exit &&
            (exit.code !== 0 && !(exit.code === null &&
              (exit.signal === "SIGTERM" || (forced && exit.signal === "SIGKILL"))))) {
          throw new Error(`Chromium exited unexpectedly during shutdown (code ${exit.code}, signal ${exit.signal ?? "none"})`);
        }
      })();
    },
  };
}

export async function waitForDevTools(browser, profile, signal) {
  const path = join(profile, "DevToolsActivePort");
  while (true) {
    signal.throwIfAborted();
    browser.assertRunning();
    try {
      const [port, browserPath] = (await readFile(path, "utf8")).trim().split(/\r?\n/);
      // The file can be observed between its port and endpoint writes.
      if (/^\d+$/.test(port) && Number(port) > 0 && Number(port) <= 65535 &&
          /^\/devtools\/browser\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(browserPath ?? "")) {
        return `ws://127.0.0.1:${port}${browserPath}`;
      }
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    await delay(50, undefined, { signal });
  }
}

export async function cleanupBrowser(browser, profile, options) {
  // If stop fails, preserve the profile: the owned process may still be writing.
  if (browser) await browser.stop(options);
  if (profile) await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
