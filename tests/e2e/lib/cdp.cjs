"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");

const EDGE_CANDIDATES = [
  process.env.NADA_EDGE_PATH,
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
  process.env.LOCALAPPDATA &&
    path.join(process.env.LOCALAPPDATA, "Microsoft\\Edge\\Application\\msedge.exe"),
].filter(Boolean);

function findEdgePath() {
  for (const p of EDGE_CANDIDATES) {
    if (p && fs.existsSync(p)) return p;
  }
  throw new Error(
    `msedge.exe not found. Checked: ${EDGE_CANDIDATES.join(", ")}`
  );
}

const activeSessions = new Set();

function cleanupAllSync() {
  for (const session of [...activeSessions]) {
    try {
      session.forceKillSync();
    } catch (_) {}
  }
}

process.on("exit", cleanupAllSync);

class CdpSession {
  constructor(proc, ws, userDataDir) {
    this.proc = proc;
    this.ws = ws;
    this.userDataDir = userDataDir;
    this.nextId = 1;
    this.pending = new Map();
    this.eventListeners = new Map();
    this.sessionId = null;
    this.targetId = null;
    this.closed = false;
    this.consoleErrors = [];
    this.pageErrors = [];
    activeSessions.add(this);

    ws.addEventListener("message", (evt) => {
      let msg;
      try {
        msg = JSON.parse(typeof evt.data === "string" ? evt.data : String(evt.data));
      } catch (_) {
        return;
      }
      if (msg.id && this.pending.has(msg.id)) {
        const entry = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        clearTimeout(entry.timer);
        if (msg.error) {
          entry.reject(
            new Error(`CDP ${entry.method} error ${msg.error.code}: ${msg.error.message}`)
          );
        } else {
          entry.resolve(msg.result || {});
        }
        return;
      }
      if (msg.method) {
        if (msg.method === "Runtime.exceptionThrown") {
          const d = msg.params && msg.params.exceptionDetails;
          const text =
            (d && d.exception && (d.exception.description || d.exception.value)) ||
            (d && d.text) ||
            "Unknown Runtime.exceptionThrown";
          this.pageErrors.push(text);
        } else if (
          msg.method === "Runtime.consoleAPICalled" &&
          msg.params &&
          msg.params.type === "error"
        ) {
          const args = (msg.params.args || []).map((a) =>
            a.value !== undefined ? String(a.value) : a.description || a.type
          );
          this.consoleErrors.push(args.join(" "));
        }
        const handlers = this.eventListeners.get(msg.method);
        if (handlers) {
          for (const fn of [...handlers]) {
            try {
              fn(msg.params || {}, msg.sessionId || null);
            } catch (_) {}
          }
        }
      }
    });
  }

  on(method, handler) {
    if (!this.eventListeners.has(method)) {
      this.eventListeners.set(method, new Set());
    }
    this.eventListeners.get(method).add(handler);
    return () => this.eventListeners.get(method)?.delete(handler);
  }

  once(method, predicate = () => true, timeoutMs = 15000) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        off();
        reject(new Error(`Timed out waiting for CDP event ${method} (${timeoutMs}ms)`));
      }, timeoutMs);
      const off = this.on(method, (params, sid) => {
        if (!sid || sid === this.sessionId) {
          if (predicate(params)) {
            clearTimeout(timer);
            off();
            resolve(params);
          }
        }
      });
    });
  }

  send(method, params = {}, { sessionId = this.sessionId, timeoutMs = 15000 } = {}) {
    if (this.closed) {
      return Promise.reject(new Error(`CDP session closed before ${method}`));
    }
    const id = this.nextId++;
    const payload = { id, method, params };
    if (sessionId) payload.sessionId = sessionId;

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP ${method} timed out after ${timeoutMs}ms`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer, method });
      this.ws.send(JSON.stringify(payload));
    });
  }

  async initPage({ width = 430, height = 640, initScript = null } = {}) {
    const { targetId } = await this.send(
      "Target.createTarget",
      { url: "about:blank" },
      { sessionId: null }
    );
    this.targetId = targetId;
    const { sessionId } = await this.send(
      "Target.attachToTarget",
      { targetId, flatten: true },
      { sessionId: null }
    );
    this.sessionId = sessionId;
    await this.send("Page.enable");
    await this.send("Runtime.enable");
    await this.send("DOM.enable");
    await this.setViewport(width, height);
    if (initScript) {
      await this.addInitScript(initScript);
    }
  }

  async setViewport(width, height) {
    await this.send("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
  }

  async addInitScript(source) {
    await this.send("Page.addScriptToEvaluateOnNewDocument", { source });
  }

  async navigate(url, timeoutMs = 15000) {
    const loaded = this.once("Page.loadEventFired", () => true, timeoutMs);
    await this.send("Page.navigate", { url });
    await loaded;
  }

  async evaluate(expression, { awaitPromise = true, timeoutMs = 15000 } = {}) {
    const expr =
      typeof expression === "function" ? `(${expression.toString()})()` : String(expression);
    const res = await this.send(
      "Runtime.evaluate",
      {
        expression: expr,
        awaitPromise,
        returnByValue: true,
        userGesture: true,
      },
      { timeoutMs }
    );
    if (res.exceptionDetails) {
      const ex = res.exceptionDetails;
      const msg =
        (ex.exception && (ex.exception.description || ex.exception.value)) ||
        ex.text ||
        "Evaluation failed";
      throw new Error(`Browser evaluate error: ${msg}`);
    }
    return res.result ? res.result.value : undefined;
  }

  async waitFor(fnOrExpr, { timeoutMs = 15000, intervalMs = 100, label = "condition" } = {}) {
    const deadline = Date.now() + timeoutMs;
    const expr =
      typeof fnOrExpr === "function" ? `(${fnOrExpr.toString()})()` : String(fnOrExpr);
    let lastErr = null;
    while (Date.now() < deadline) {
      try {
        const val = await this.evaluate(expr, { awaitPromise: true, timeoutMs: 5000 });
        if (val) return val;
      } catch (err) {
        lastErr = err;
      }
      await new Promise((r) => setTimeout(r, intervalMs));
    }
    throw new Error(
      `Timed out waiting for ${label} after ${timeoutMs}ms${lastErr ? ` (last error: ${lastErr.message})` : ""}`
    );
  }

  async dispatchMouseClick(x, y, { button = "left", clickCount = 1 } = {}) {
    const buttons = button === "right" ? 2 : 1;
    await this.send("Input.dispatchMouseEvent", {
      type: "mouseMoved",
      x,
      y,
    });
    await this.send("Input.dispatchMouseEvent", {
      type: "mousePressed",
      x,
      y,
      button,
      buttons,
      clickCount,
    });
    await this.send("Input.dispatchMouseEvent", {
      type: "mouseReleased",
      x,
      y,
      button,
      buttons: 0,
      clickCount,
    });
  }

  async clickSelector(selector, options = {}) {
    const box = await this.evaluate(`(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return null;
      el.scrollIntoView({ block: 'center', inline: 'center' });
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height };
    })()`);
    if (!box || box.w <= 0 || box.h <= 0) {
      throw new Error(`Cannot click selector ${selector}: missing or zero-size bounding box`);
    }
    await this.dispatchMouseClick(box.x, box.y, options);
    return box;
  }

  async pressKey(key, code = key) {
    await this.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      key,
      code,
    });
    await this.send("Input.dispatchKeyEvent", {
      type: "keyUp",
      key,
      code,
    });
  }

  forceKillSync() {
    this.closed = true;
    activeSessions.delete(this);
    for (const [, entry] of this.pending) {
      clearTimeout(entry.timer);
      entry.reject(new Error("CDP session force-closed"));
    }
    this.pending.clear();
    try {
      this.ws.close();
    } catch (_) {}
    if (this.proc && this.proc.pid) {
      if (process.platform === "win32") {
        try {
          spawnSync("taskkill", ["/PID", String(this.proc.pid), "/T", "/F"], {
            stdio: "ignore",
            windowsHide: true,
          });
        } catch (_) {}
      } else {
        try {
          process.kill(-this.proc.pid, "SIGKILL");
        } catch (_) {
          try {
            this.proc.kill("SIGKILL");
          } catch (_) {}
        }
      }
    }
    if (this.userDataDir && fs.existsSync(this.userDataDir)) {
      for (let i = 0; i < 5; i++) {
        try {
          fs.rmSync(this.userDataDir, { recursive: true, force: true });
          break;
        } catch (_) {}
      }
    }
  }

  async close() {
    if (this.closed) return;
    this.closed = true;
    activeSessions.delete(this);
    for (const [, entry] of this.pending) {
      clearTimeout(entry.timer);
      entry.reject(new Error("CDP session closed"));
    }
    this.pending.clear();

    try {
      if (this.ws.readyState === WebSocket.OPEN) {
        await Promise.race([
          this.send("Browser.close", {}, { sessionId: null, timeoutMs: 2000 }).catch(() => {}),
          new Promise((r) => setTimeout(r, 1500)),
        ]);
      }
    } catch (_) {}

    try {
      this.ws.close();
    } catch (_) {}
    try { this.proc?.stdout?.destroy(); } catch (_) {}
    try { this.proc?.stderr?.destroy(); } catch (_) {}

    if (this.proc && this.proc.exitCode === null) {
      await new Promise((resolve) => {
        const timer = setTimeout(() => {
          if (process.platform === "win32" && this.proc.pid) {
            spawnSync("taskkill", ["/PID", String(this.proc.pid), "/T", "/F"], {
              stdio: "ignore",
              windowsHide: true,
            });
          } else {
            try {
              this.proc.kill("SIGKILL");
            } catch (_) {}
          }
          resolve();
        }, 400);
        this.proc.once("exit", () => {
          clearTimeout(timer);
          resolve();
        });
      });
    } else if (process.platform === "win32" && this.proc && this.proc.pid) {
      // Ensure any child crashpad/GPU processes in the process tree are also terminated
      try {
        spawnSync("taskkill", ["/PID", String(this.proc.pid), "/T", "/F"], {
          stdio: "ignore",
          windowsHide: true,
        });
      } catch (_) {}
    }

    if (this.userDataDir && fs.existsSync(this.userDataDir)) {
      for (let i = 0; i < 25; i++) {
        try {
          fs.rmSync(this.userDataDir, { recursive: true, force: true });
          break;
        } catch (_) {
          if (i === 5 && process.platform === "win32") {
            const tag = path.basename(this.userDataDir).replace(/'/g, "");
            try {
              spawnSync(
                "powershell.exe",
                [
                  "-NoProfile",
                  "-Command",
                  `Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" | Where-Object { $_.CommandLine -like '*${tag}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`,
                ],
                { stdio: "ignore", windowsHide: true }
              );
            } catch (_) {}
          }
          await new Promise((r) => setTimeout(r, 100));
        }
      }
    }
  }
}

async function launchEdge({ width = 430, height = 640, initScript = null, timeoutMs = 25000 } = {}) {
  const edgePath = findEdgePath();
  const userDataDir = fs.mkdtempSync(path.join(os.tmpdir(), "nada-e2e-edge-"));
  const args = [
    "--headless=new",
    "--disable-gpu",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
    "--disable-crash-reporter",
    "--noerrdialogs",
    "--autoplay-policy=no-user-gesture-required",
    "--remote-debugging-port=0",
    `--window-size=${width},${height}`,
    `--user-data-dir=${userDataDir}`,
    "about:blank",
  ];

  const proc = spawn(edgePath, args, {
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });

  let wsUrl;
  try {
    wsUrl = await new Promise((resolve, reject) => {
      let output = "";
      const timer = setTimeout(() => {
        reject(new Error(`Edge CDP startup timed out after ${timeoutMs}ms. Output:\n${output}`));
      }, timeoutMs);

      const onChunk = (chunk) => {
        output += chunk.toString("utf8");
        const m = output.match(/DevTools listening on (ws:\/\/[^\s\r\n]+)/);
        if (m) {
          clearTimeout(timer);
          proc.stderr.off("data", onChunk);
          proc.stdout.off("data", onChunk);
          resolve(m[1]);
        }
      };
      proc.stderr.on("data", onChunk);
      proc.stdout.on("data", onChunk);
      proc.once("exit", (code) => {
        clearTimeout(timer);
        reject(new Error(`Edge exited early with code ${code}. Output:\n${output}`));
      });
    });

    const ws = new WebSocket(wsUrl);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("WebSocket connect timeout")), 5000);
      ws.addEventListener("open", () => {
        clearTimeout(timer);
        resolve();
      });
      ws.addEventListener("error", (err) => {
        clearTimeout(timer);
        reject(new Error(`WebSocket error: ${err.message || "failed"}`));
      });
    });

    const session = new CdpSession(proc, ws, userDataDir);
    await session.initPage({ width, height, initScript });
    return session;
  } catch (err) {
    if (process.platform === "win32" && proc.pid) {
      spawnSync("taskkill", ["/PID", String(proc.pid), "/T", "/F"], {
        stdio: "ignore",
        windowsHide: true,
      });
    } else {
      try {
        proc.kill("SIGKILL");
      } catch (_) {}
    }
    try {
      fs.rmSync(userDataDir, { recursive: true, force: true });
    } catch (_) {}
    throw err;
  }
}

module.exports = {
  findEdgePath,
  launchEdge,
  CdpSession,
};
