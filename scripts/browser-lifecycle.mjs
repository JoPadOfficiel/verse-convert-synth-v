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

export function launchBrowserProcess(command, args) {
  const child = spawn(command, args, { stdio: ["ignore", "ignore", "pipe"] });
  let stderr = Buffer.alloc(0);
  let closed = false;
  let failure;
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
  child.once("exit", (code, signal) => fail(`Chromium exited (code ${code}, signal ${signal ?? "none"})`));
  const completion = new Promise((resolve) => {
    // 'close' follows exit/spawn failure and closure of the stderr pipe.
    child.once("close", (code, signal) => { closed = true; resolve({ code, signal }); });
  });
  return {
    child,
    completion,
    diagnostics,
    guard: (operation) => Promise.race([operation, failed]),
    assertRunning() { if (failure) throw failure; },
    async stop({ stopTimeoutMs = 5_000, log = console.error } = {}) {
      if (closed) return;
      if (child.pid && child.exitCode === null && child.signalCode === null) child.kill("SIGTERM");
      const graceful = await withDeadline(() => completion.then(() => true), stopTimeoutMs, "Chromium graceful shutdown")
        .catch((error) => { log(error.message); return false; });
      if (graceful) return;
      log("Force-stopping owned Chromium child with SIGKILL");
      if (child.pid && child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
      await withDeadline(() => completion, stopTimeoutMs, "Chromium forced shutdown");
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
      if (/^\d+$/.test(port) && Number(port) > 0 && Number(port) <= 65535 && browserPath?.startsWith("/devtools/browser/")) {
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
