// Run the real rendered App in an isolated headless Chromium profile. Native
// Tauri IPC/dialogs remain mocked by tests/pronunciation-browser.tsx.
import { createServer } from "vite";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { cleanupBrowser, launchBrowserProcess, timeoutSetting, waitForDevTools, withDeadline } from "./browser-lifecycle.mjs";

const report = resolve(process.env.VERSE_BROWSER_REPORT ?? "src-tauri/target/pronunciation-browser/results.json");
const timeoutMs = timeoutSetting("VERSE_BROWSER_TIMEOUT_MS", 60_000);
const startupTimeoutMs = timeoutSetting("VERSE_BROWSER_STARTUP_TIMEOUT_MS", 30_000);
const stopTimeoutMs = timeoutSetting("VERSE_BROWSER_STOP_TIMEOUT_MS", 5_000);
let settleReceipt;
let rejectReceipt;
const receipt = new Promise((resolve, reject) => {
  settleReceipt = resolve;
  rejectReceipt = reject;
});
receipt.catch(() => {});

function executable() {
  const explicit = process.env.VERSE_TEST_CHROME || process.env.VERSE_BROWSER_BIN || process.env.CHROME_BIN;
  if (explicit) return explicit;
  if (process.platform === "darwin") {
    return "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  }
  for (const name of ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]) {
    try {
      return execFileSync("which", [name], { encoding: "utf8" }).trim();
    } catch {
      // Try the next common Chromium executable.
    }
  }
  throw new Error("No Chromium browser found. Set VERSE_TEST_CHROME, VERSE_BROWSER_BIN, or CHROME_BIN.");
}

function cdp(socket) {
  let id = 0;
  const pending = new Map();
  let failure;
  const fail = (error) => {
    failure ??= error;
    for (const current of pending.values()) current.reject(failure);
    pending.clear();
  };
  const onClose = () => fail(new Error("Chromium DevTools socket closed"));
  const onError = () => fail(new Error("Chromium DevTools socket error"));
  const onMessage = (event) => {
    try {
      const message = JSON.parse(event.data.toString());
      if (message.id) {
        const current = pending.get(message.id);
        if (!current) return;
        pending.delete(message.id);
        if (message.error) current.reject(new Error(message.error.message));
        else current.resolve(message.result);
        return;
      }
    } catch (error) { fail(error); }
  };
  socket.addEventListener("message", onMessage);
  socket.addEventListener("close", onClose);
  socket.addEventListener("error", onError);
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    if (failure) { reject(failure); return; }
    const messageId = ++id;
    pending.set(messageId, { resolve, reject });
    try {
      socket.send(JSON.stringify({ id: messageId, method, params, ...(sessionId ? { sessionId } : {}) }));
    } catch (error) {
      pending.delete(messageId);
      reject(error);
    }
  });
  return { send, dispose() {
    fail(new Error("Chromium DevTools session disposed"));
    socket.removeEventListener("message", onMessage);
    socket.removeEventListener("close", onClose);
    socket.removeEventListener("error", onError);
  } };
}

async function waitForHarness(send, sessionId, signal) {
  while (true) {
    signal.throwIfAborted();
    try {
      const state = await send("Runtime.evaluate", {
        expression: 'document.readyState === "complete" && document.getElementById("run-tests") !== null',
        returnByValue: true,
      }, sessionId);
      if (state?.result?.value === true) return;
    } catch {
      // Navigation may temporarily destroy the execution context.
    }
    await delay(50, undefined, { signal });
  }
}

await rm(report, { force: true });
const server = await createServer({
  server: { host: "127.0.0.1", port: 1428, strictPort: true },
  plugins: [{
    name: "pronunciation-test-receipt",
    configureServer(viteServer) {
      viteServer.middlewares.use("/__pronunciation-results", async (request, response) => {
        if (request.method !== "POST") { response.writeHead(405).end(); return; }
        try {
          let body = "";
          for await (const chunk of request) {
            body += chunk;
            if (body.length > 64 * 1024) { response.writeHead(413).end(); return; }
          }
          const result = JSON.parse(body);
          if (!Array.isArray(result.tests) || typeof result.passed !== "boolean") throw new Error("invalid test receipt");
          await mkdir(dirname(report), { recursive: true });
          await writeFile(report, JSON.stringify(result, null, 2) + "\n");
          response.writeHead(200, { "Content-Type": "application/json" }).end('{"saved":true}');
          settleReceipt(result);
        } catch (error) {
          response.writeHead(400).end("Invalid result");
          rejectReceipt(error);
        }
      });
    },
  }],
});

let browser;
let profile;
let socket;
let protocol;
let testError;
try {
  await server.listen();
  profile = await mkdtemp(join(tmpdir(), "verse-pronunciation-browser-"));
  browser = launchBrowserProcess(executable(), [
    "--headless=new",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-extensions",
    "--disable-sync",
    "--metrics-recording-only",
    "--no-first-run",
    "--no-default-browser-check",
    "--remote-debugging-port=0",
    "--remote-allow-origins=*",
    `--user-data-dir=${profile}`,
    "about:blank",
  ]);
  // An IPC-connected test supervisor can clean this separately owned group if
  // the harness itself stalls. Normal CLI runs have no IPC channel.
  if (process.connected) process.send({ type: "verse-browser-owned", pid: browser.child.pid });

  let stage = "DevToolsActivePort";
  await withDeadline((signal) => browser.guard((async () => {
    const endpoint = await waitForDevTools(browser, profile, signal);
    signal.throwIfAborted();
    stage = "DevTools connection";
    socket = new WebSocket(endpoint);
    // Keep transport errors observed even while connecting or closing.
    socket.addEventListener("error", () => {});
    await new Promise((resolve, reject) => {
      const finish = (error) => {
        socket.removeEventListener("open", onOpen);
        socket.removeEventListener("error", onError);
        socket.removeEventListener("close", onClose);
        signal.removeEventListener("abort", onAbort);
        if (error) reject(error); else resolve();
      };
      const onOpen = () => finish();
      const onError = () => finish(new Error("Could not connect to Chromium DevTools"));
      const onClose = () => finish(new Error("Chromium DevTools closed before connecting"));
      const onAbort = () => finish(signal.reason);
      socket.addEventListener("open", onOpen, { once: true });
      socket.addEventListener("error", onError, { once: true });
      socket.addEventListener("close", onClose, { once: true });
      signal.addEventListener("abort", onAbort, { once: true });
    });
    signal.throwIfAborted();
    protocol = cdp(socket);
    const { send } = protocol;
    stage = "harness navigation/readiness";
    const { targetId } = await send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
    await send("Page.enable", {}, sessionId);
    await send("Runtime.enable", {}, sessionId);
    await send("Page.navigate", { url: "http://127.0.0.1:1428/tests/pronunciation-browser.html" }, sessionId);
    await waitForHarness(send, sessionId, signal);
    const clicked = await send("Runtime.evaluate", {
      expression: 'document.getElementById("run-tests")?.click(); true',
      returnByValue: true,
    }, sessionId);
    if (clicked?.result?.value !== true) throw new Error("Could not start pronunciation browser regression");
  })()), startupTimeoutMs, "Chromium startup").catch((error) => {
    throw new Error(`${error.message} (stage: ${stage})`, { cause: error });
  });

  const outcome = await withDeadline(() => browser.guard(receipt), timeoutMs, "Browser regression");
  console.log(JSON.stringify({ report, passed: outcome.passed, tests: outcome.tests.length, reloads: outcome.reloads ?? null }));
  if (!outcome.passed) {
    throw new Error(outcome.error ?? "Pronunciation browser regression failed");
  }
  if (outcome.tests.length !== 24 || outcome.tests.some((test) => test.passed !== true) || outcome.reloads !== 3) {
    throw new Error("Incomplete browser receipt: expected 24 passing cases and 3 reloads");
  }
} catch (error) {
  testError = error;
  process.exitCode = 1;
} finally {
  const cleanupErrors = [];
  if (protocol && browser) {
    let closed;
    try {
      // Let Chrome drain/reap its helpers before falling back to OS signals.
      // https://chromedevtools.github.io/devtools-protocol/tot/Browser/#method-close
      closed = await withDeadline(async () => {
        // Some versions close the socket before acknowledging Browser.close.
        try { await protocol.send("Browser.close"); } catch {}
        return browser.completion;
      }, stopTimeoutMs, "Chromium DevTools shutdown");
    } catch (error) {
      console.error(`${error.message}; falling back to owned-process shutdown`);
    }
    if (closed && closed.code !== 0) {
      cleanupErrors.push(new Error(`Chromium exited unexpectedly during DevTools shutdown (code ${closed.code}, signal ${closed.signal ?? "none"})`));
    }
  }
  try {
    protocol?.dispose();
    socket?.close();
  } catch (error) { cleanupErrors.push(error); }
  try {
    await cleanupBrowser(browser, profile, { stopTimeoutMs });
  } catch (error) { cleanupErrors.push(error); }
  try {
    server.httpServer?.closeAllConnections();
    await withDeadline(() => server.close(), stopTimeoutMs, "Vite shutdown");
  } catch (error) { cleanupErrors.push(error); }
  if (testError) console.error(testError.stack ?? String(testError));
  for (const error of cleanupErrors) {
    console.error("Browser cleanup failed:", error.stack ?? String(error));
    process.exitCode = 1;
  }
  if (process.exitCode) console.error(browser?.diagnostics() ?? "Chromium was not started");
}
