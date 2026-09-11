// Run the real rendered App in an isolated headless Chromium profile. Native
// Tauri IPC/dialogs remain mocked by tests/pronunciation-browser.tsx.
import { createServer } from "vite";
import { execFileSync, spawn } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

const report = resolve(process.env.VERSE_BROWSER_REPORT ?? "src-tauri/target/pronunciation-browser/results.json");
const timeoutMs = Number(process.env.VERSE_BROWSER_TIMEOUT_MS ?? 60_000);
let settleReceipt;
let rejectReceipt;
const receipt = new Promise((resolve, reject) => {
  settleReceipt = resolve;
  rejectReceipt = reject;
});

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

async function waitForFile(path, deadline) {
  while (Date.now() < deadline) {
    try {
      return await readFile(path, "utf8");
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  }
  throw new Error(`Timed out waiting for ${path}`);
}

function cdp(socket) {
  let id = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data.toString());
    if (message.id) {
      const current = pending.get(message.id);
      if (!current) return;
      pending.delete(message.id);
      if (message.error) current.reject(new Error(message.error.message));
      else current.resolve(message.result);
      return;
    }
  });
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const messageId = ++id;
    pending.set(messageId, { resolve, reject });
    socket.send(JSON.stringify({ id: messageId, method, params, ...(sessionId ? { sessionId } : {}) }));
  });
  return { send };
}

async function waitForHarness(send, sessionId, deadline) {
  while (Date.now() < deadline) {
    try {
      const state = await send("Runtime.evaluate", {
        expression: 'document.readyState === "complete" && document.getElementById("run-tests") !== null',
        returnByValue: true,
      }, sessionId);
      if (state?.result?.value === true) return;
    } catch {
      // Navigation may temporarily destroy the execution context.
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("Timed out waiting for pronunciation browser harness");
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
try {
  await server.listen();
  profile = await mkdtemp(join(tmpdir(), "verse-pronunciation-browser-"));
  browser = spawn(executable(), [
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
  ], { stdio: "ignore" });

  const activePort = await waitForFile(join(profile, "DevToolsActivePort"), Date.now() + 10_000);
  const [port, browserPath] = activePort.trim().split(/\r?\n/);
  const socket = new WebSocket(`ws://127.0.0.1:${port}${browserPath}`);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  const { send } = cdp(socket);
  const { targetId } = await send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
  await send("Page.enable", {}, sessionId);
  await send("Runtime.enable", {}, sessionId);
  await send("Page.navigate", { url: "http://127.0.0.1:1428/tests/pronunciation-browser.html" }, sessionId);
  await waitForHarness(send, sessionId, Date.now() + 10_000);
  const clicked = await send("Runtime.evaluate", {
    expression: 'document.getElementById("run-tests")?.click(); true',
    returnByValue: true,
  }, sessionId);
  if (clicked?.result?.value !== true) throw new Error("Could not start pronunciation browser regression");

  const outcome = await Promise.race([
    receipt,
    new Promise((_, reject) => setTimeout(
      () => reject(new Error(`Browser regression timed out after ${timeoutMs} ms`)), timeoutMs
    )),
  ]);
  console.log(JSON.stringify({ report, passed: outcome.passed, tests: outcome.tests.length, reloads: outcome.reloads ?? null }));
  if (!outcome.passed) {
    console.error(outcome.error ?? "Pronunciation browser regression failed");
    process.exitCode = 1;
  }
  socket.close();
} catch (error) {
  console.error(error instanceof Error ? error.stack ?? error.message : String(error));
  process.exitCode = 1;
} finally {
  if (browser && browser.exitCode === null) browser.kill("SIGTERM");
  await server.close();
  if (profile) await rm(profile, { recursive: true, force: true });
}
