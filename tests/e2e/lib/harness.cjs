"use strict";

const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { launchEdge } = require("./cdp.cjs");
const { getStandardFixtures } = require("./media_gen.cjs");

const REPO_ROOT = path.resolve(__dirname, "..", "..", "..");
const SRC_DIR = path.join(REPO_ROOT, "src");

const MIME_BY_EXT = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".json": "application/json; charset=utf-8",
};

function parseRangeHeader(rangeHeader, total) {
  if (!rangeHeader || typeof rangeHeader !== "string") {
    return { isRange: false, start: 0, end: total - 1, valid: true };
  }
  const m = /^bytes=(\d*)-(\d*)$/i.exec(rangeHeader.trim());
  if (!m) return { isRange: true, start: 0, end: total - 1, valid: false };
  const [, left, right] = m;
  if (left === "" && right === "") {
    return { isRange: true, start: 0, end: 0, valid: false };
  }
  let start, end;
  if (left === "") {
    const suffix = Number(right);
    start = Math.max(0, total - suffix);
    end = total - 1;
  } else {
    start = Number(left);
    end = right === "" ? total - 1 : Math.min(total - 1, Number(right));
  }
  if (!Number.isFinite(start) || !Number.isFinite(end) || start >= total || start > end) {
    return { isRange: true, start, end, valid: false };
  }
  return { isRange: true, start, end, valid: true };
}

function createAppServer({
  ipcHandlers = {},
  cspHeader = null,
  serveMediaWith206OnGet = true,
} = {}) {
  const mediaRegistry = new Map();
  const mediaRequests = [];
  const invokeLog = [];
  const handlers = {
    home_section: async () => ({ items: [] }),
    search: async () => [],
    collection_tracks: async () => [],
    streams: async () => ({ muxed: [], video_only: [], audio: [], expires_in_seconds: 21600 }),
    radio: async () => [],
    set_tray_title: async () => null,
    ...ipcHandlers,
  };

  function registerMedia(token, fixture, fault = {}) {
    mediaRegistry.set(token, {
      fixture,
      fault: { ...fault },
      hitCount: 0,
    });
    return token;
  }

  function updateMediaFault(token, fault) {
    const entry = mediaRegistry.get(token);
    if (!entry) throw new Error(`Unknown media token: ${token}`);
    entry.fault = { ...fault };
  }

  function setIpcHandler(cmd, fn) {
    handlers[cmd] = fn;
  }

  function readBody(req) {
    return new Promise((resolve, reject) => {
      const chunks = [];
      req.on("data", (c) => chunks.push(c));
      req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
      req.on("error", reject);
    });
  }

  const server = http.createServer(async (req, res) => {
    const urlObj = new URL(req.url, "http://127.0.0.1");
    const pathname = urlObj.pathname;

    if (pathname === "/__ipc" && req.method === "POST") {
      try {
        const body = JSON.parse(await readBody(req));
        const cmd = body.cmd;
        const args = body.args || {};
        invokeLog.push({ cmd, args, at: Date.now() });
        const fn = handlers[cmd];
        if (!fn) {
          res.writeHead(400, { "Content-Type": "application/json" });
          res.end(JSON.stringify({ ok: false, error: `Unknown command: ${cmd}` }));
          return;
        }
        const result = await fn(args, { server, mediaUrl: (t) => `${baseUrl()}/media/${t}` });
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ ok: true, result: result === undefined ? null : result }));
      } catch (err) {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ ok: false, error: String(err && err.message ? err.message : err) }));
      }
      return;
    }

    if (pathname.startsWith("/media/")) {
      const token = pathname.slice("/media/".length);
      const entry = mediaRegistry.get(token);
      if (!entry) {
        mediaRequests.push({
          token,
          method: req.method,
          range: req.headers.range || null,
          status: 404,
          bytesSent: 0,
          at: Date.now(),
        });
        res.writeHead(404);
        res.end("not found");
        return;
      }

      entry.hitCount++;
      const { fixture, fault } = entry;
      const total = fixture.buffer.length;
      const parsed = parseRangeHeader(req.headers.range, total);

      const logEntry = {
        token,
        hit: entry.hitCount,
        method: req.method,
        range: req.headers.range || null,
        start: parsed.start,
        end: parsed.end,
        status: 200,
        bytesSent: 0,
        at: Date.now(),
      };
      mediaRequests.push(logEntry);

      if (fault.errorStatus && (!fault.errorOnHit || entry.hitCount === fault.errorOnHit)) {
        if (!fault.errorAfterBytes || parsed.start >= fault.errorAfterBytes) {
          logEntry.status = fault.errorStatus;
          res.writeHead(fault.errorStatus, { "Content-Type": "text/plain" });
          res.end(`Injected ${fault.errorStatus}`);
          return;
        }
      }

      if (fault.failOnNonZeroRange && parsed.isRange && parsed.start > 0) {
        logEntry.status = 403;
        res.writeHead(403, { "Content-Type": "text/plain" });
        res.end("PO token range blocked");
        return;
      }

      if (!parsed.valid) {
        logEntry.status = 416;
        res.writeHead(416, { "Content-Range": `bytes */${total}` });
        res.end();
        return;
      }

      const slice = fixture.buffer.subarray(parsed.start, parsed.end + 1);
      const status = parsed.isRange || serveMediaWith206OnGet ? 206 : 200;
      logEntry.status = status;

      const headers = {
        "Content-Type": fixture.mime,
        "Accept-Ranges": "bytes",
        "Content-Length": String(slice.length),
        "Cache-Control": "no-store",
      };
      if (status === 206) {
        headers["Content-Range"] = `bytes ${parsed.start}-${parsed.end}/${total}`;
      }

      if (req.method === "HEAD") {
        res.writeHead(status, headers);
        res.end();
        return;
      }

      // Fault injection during streaming body
      const hasMidFault =
        (typeof fault.dropAfterBytes === "number" && parsed.start < fault.dropAfterBytes) ||
        (typeof fault.stallAfterBytes === "number" && fault.stallMs > 0);

      if (!hasMidFault) {
        res.writeHead(status, headers);
        res.end(slice);
        logEntry.bytesSent = slice.length;
        return;
      }

      res.writeHead(status, headers);
      let cursor = 0;
      const chunkSz = 32 * 1024;
      let stalledOnce = false;

      const pump = () => {
        if (res.destroyed || req.destroyed) return;
        const absPos = parsed.start + cursor;

        if (
          typeof fault.dropAfterBytes === "number" &&
          absPos >= fault.dropAfterBytes
        ) {
          req.socket.destroy();
          return;
        }

        if (
          !stalledOnce &&
          typeof fault.stallAfterBytes === "number" &&
          absPos >= fault.stallAfterBytes &&
          fault.stallMs > 0
        ) {
          stalledOnce = true;
          setTimeout(pump, fault.stallMs);
          return;
        }

        if (cursor >= slice.length) {
          res.end();
          return;
        }

        let nextEnd = Math.min(slice.length, cursor + chunkSz);
        if (typeof fault.dropAfterBytes === "number") {
          const maxCursor = Math.max(0, fault.dropAfterBytes - parsed.start);
          if (maxCursor > cursor && maxCursor < nextEnd) {
            nextEnd = maxCursor;
          }
        }

        const chunk = slice.subarray(cursor, nextEnd);
        cursor = nextEnd;
        logEntry.bytesSent += chunk.length;
        const ok = res.write(chunk);
        if (ok) {
          setImmediate(pump);
        } else {
          res.once("drain", pump);
        }
      };

      pump();
      return;
    }

    // Serve static files from unmodified src/
    const rel = pathname === "/" ? "index.html" : pathname.replace(/^\/+/, "");
    const abs = path.resolve(SRC_DIR, rel);
    if (!abs.startsWith(SRC_DIR) || !fs.existsSync(abs) || fs.statSync(abs).isDirectory()) {
      res.writeHead(404);
      res.end("Not found");
      return;
    }
    const ext = path.extname(abs).toLowerCase();
    const hdrs = {
      "Content-Type": MIME_BY_EXT[ext] || "application/octet-stream",
      "Cache-Control": "no-store",
    };
    if (cspHeader && ext === ".html") {
      hdrs["Content-Security-Policy"] = cspHeader;
    }
    res.writeHead(200, hdrs);
    fs.createReadStream(abs).pipe(res);
  });

  function baseUrl() {
    const addr = server.address();
    return `http://127.0.0.1:${addr.port}`;
  }

  return {
    server,
    start: () =>
      new Promise((resolve) => {
        server.listen(0, "127.0.0.1", () => resolve(baseUrl()));
      }),
    close: () =>
      new Promise((resolve) => {
        try { server.closeAllConnections(); } catch (_) {}
        server.close(() => resolve());
      }),
    baseUrl,
    registerMedia,
    updateMediaFault,
    setIpcHandler,
    getInvokeLog: () => [...invokeLog],
    clearInvokeLog: () => { invokeLog.length = 0; },
    getMediaRequests: (token = null) =>
      token ? mediaRequests.filter((r) => r.token === token) : [...mediaRequests],
    clearMediaRequests: () => { mediaRequests.length = 0; },
  };
}

function buildTauriInitScript(baseUrl, { stubYT = false, breakLocalStorage = false } = {}) {
  return `
    (() => {
      const ipcUrl = ${JSON.stringify(`${baseUrl}/__ipc`)};
      const eventListeners = new Map();
      window.__NADA_TEST_EVENTS__ = {
        emit(name, payload) {
          const set = eventListeners.get(name);
          if (!set) return;
          for (const cb of set) cb({ event: name, payload });
        },
        listeners: eventListeners,
      };
      window.__TAURI__ = {
        core: {
          async invoke(cmd, args) {
            const resp = await fetch(ipcUrl, {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ cmd, args: args || {} }),
            });
            const data = await resp.json();
            if (!data.ok) throw new Error(data.error || "IPC error");
            return data.result;
          },
        },
        event: {
          async listen(name, cb) {
            if (!eventListeners.has(name)) eventListeners.set(name, new Set());
            eventListeners.get(name).add(cb);
            return () => eventListeners.get(name)?.delete(cb);
          },
        },
      };
      ${breakLocalStorage ? `
        Object.defineProperty(window, "localStorage", {
          configurable: true,
          get() { throw new DOMException("Storage disabled", "SecurityError"); }
        });
      ` : ""}
      ${stubYT ? `
        window.__NADA_YT_LOADS__ = [];
        window.__NADA_YT_READY_FIRED__ = false;
        window.YT = {
          PlayerState: { ENDED: 0, PLAYING: 1, PAUSED: 2, BUFFERING: 3 },
          Player: function(id, cfg) {
            this.id = id;
            this.cfg = cfg;
            setTimeout(() => {
              cfg && cfg.events && cfg.events.onReady && cfg.events.onReady();
              window.__NADA_YT_READY_FIRED__ = true;
            }, 0);
          }
        };
        window.YT.Player.prototype = {
          loadVideoById(arg) { window.__NADA_YT_LOADS__.push(arg); },
          playVideo() {},
          pauseVideo() {},
          stopVideo() {},
          setVolume() {},
          seekTo() {},
          getCurrentTime() { return 0; },
          getDuration() { return 0; },
        };
      ` : ""}
    })();
  `;
}

async function createBrowserHarness({
  width = 430,
  height = 640,
  ipcHandlers = {},
  cspHeader = null,
  stubYT = false,
  breakLocalStorage = false,
  interceptHttps = true,
  youtubeStubScript = null,
} = {}) {
  const appServer = createAppServer({ ipcHandlers, cspHeader });
  const baseUrl = await appServer.start();
  const initScript = buildTauriInitScript(baseUrl, { stubYT, breakLocalStorage });

  let edge;
  try {
    edge = await launchEdge({ width, height, initScript });

    if (interceptHttps) {
      await edge.send("Fetch.enable", {
        patterns: [{ urlPattern: "https://*", requestStage: "Request" }],
      });
      edge.on("Fetch.requestPaused", async (params) => {
        const reqUrl = params.request && params.request.url ? params.request.url : "";
        let body = "";
        let contentType = "text/plain";
        if (reqUrl.includes("fonts.googleapis.com")) {
          contentType = "text/css";
          body = "";
        } else if (reqUrl.includes("youtube.com/iframe_api")) {
          contentType = "application/javascript";
          body = youtubeStubScript !== null
            ? youtubeStubScript
            : (stubYT ? "window.onYouTubeIframeAPIReady && window.onYouTubeIframeAPIReady();" : "");
        }
        try {
          await edge.send("Fetch.fulfillRequest", {
            requestId: params.requestId,
            responseCode: 200,
            responseHeaders: [{ name: "Content-Type", value: contentType }],
            body: Buffer.from(body, "utf8").toString("base64"),
          });
        } catch (_) {}
      });
    }

    await edge.navigate(`${baseUrl}/`);
    await edge.waitFor(() => Boolean(document.getElementById("homeSections")), {
      timeoutMs: 10000,
      label: "app initial render",
    });
    if (stubYT && youtubeStubScript === null) {
      await edge.waitFor(() => Boolean(window.__NADA_YT_READY_FIRED__), {
        timeoutMs: 5000,
        label: "YouTube stub ready",
      });
    }

    return {
      baseUrl,
      edge,
      appServer,
      fixtures: getStandardFixtures(),
      registerTrackMedia(token, fixture, fault = {}) {
        appServer.registerMedia(token, fixture, fault);
        return `${baseUrl}/media/${token}`;
      },
      async close() {
        try {
          if (edge) await edge.close();
        } finally {
          await appServer.close();
        }
      },
    };
  } catch (err) {
    if (edge) await edge.close().catch(() => {});
    await appServer.close().catch(() => {});
    throw err;
  }
}

function createDomVmHarness({
  invoke = async () => {},
  homeInvoke = async () => ({ items: [] }),
  opts = {},
} = {}) {
  const elements = new Map();
  const windowHandlers = {};
  const documentHandlers = {};
  const tauriListeners = {};
  const timers = new Map();
  const intervals = [];
  let nextTimerId = 1;

  class FakeElement {
    constructor(id, tagName = "div") {
      this.id = id;
      this.tagName = tagName.toUpperCase();
      this.paused = true;
      this.currentTime = 0;
      this.duration = 200;
      this.readyState = opts.resetReadyStateOnSrc ? 0 : 4;
      this.handlers = {};
      this.children = [];
      this.dataset = {};
      this.attributes = {};
      this.value = id === "search" ? "" : 80;
      this.ended = false;
      this.seeking = false;
      this.muted = false;
      this.error = null;
      this._src = undefined;
      if (opts.resetReadyStateOnSrc) {
        Object.defineProperty(this, "src", {
          configurable: true,
          enumerable: true,
          get() { return this._src; },
          set(val) {
            this._src = val;
            this.readyState = 0;
            this.currentTime = 0;
            this.error = null;
          },
        });
      }
      const classes = new Set();
      if (id === "videoStage" || id === "ytPlayer" || id === "status" || id === "toast" || id === "repeatBadge") {
        classes.add("hidden");
      }
      this.classList = {
        add: (...names) => names.forEach((n) => classes.add(n)),
        remove: (...names) => names.forEach((n) => classes.delete(n)),
        contains: (name) => classes.has(name),
        toggle: (name, force) => {
          const add = force ?? !classes.has(name);
          if (add) classes.add(name);
          else classes.delete(name);
          return add;
        },
      };
    }
    addEventListener(type, handler, options) {
      (this.handlers[type] ??= []).push({ handler, once: Boolean(options && options.once) });
    }
    removeEventListener(type, handler) {
      if (!this.handlers[type]) return;
      this.handlers[type] = this.handlers[type].filter((e) => e.handler !== handler);
    }
    emit(type, extra = {}) {
      const list = [...(this.handlers[type] ?? [])];
      let stopped = false;
      for (const entry of list) {
        if (entry.once) this.removeEventListener(type, entry.handler);
        entry.handler({
          target: this,
          type,
          stopPropagation() { stopped = true; },
          preventDefault() {},
          ...extra,
        });
        if (stopped) break;
      }
    }
    click() { this.emit("click"); }
    append(...items) { this.children.push(...items); }
    appendChild(item) { this.children.push(item); return item; }
    setAttribute(name, value) { this.attributes[name] = value; this[name] = value; }
    getAttribute(name) { return this.attributes[name] ?? null; }
    hasAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attributes, name); }
    set textContent(value) { this.text = value; this.children = []; }
    get textContent() { return this.text || ""; }
    removeAttribute(name) {
      delete this.attributes[name];
      if (name === "src" && opts.resetReadyStateOnSrc) {
        this._src = undefined;
        this.readyState = 0;
        this.currentTime = 0;
        return;
      }
      delete this[name];
    }
    load() {}
    blur() {}
    closest() { return null; }
    pause() { this.paused = true; this.emit("pause"); }
    play() {
      this.paused = false;
      this.emit("play");
      return this.playError ? Promise.reject(this.playError) : Promise.resolve();
    }
    async requestPictureInPicture() {
      documentObj.pictureInPictureElement = this;
      this.emit("enterpictureinpicture");
    }
    canPlayType(mime) {
      if (opts.canPlayType) return opts.canPlayType(mime);
      return mime && mime.includes("unsupported") ? "" : "probably";
    }
  }

  const documentObj = {
    pictureInPictureEnabled: true,
    pictureInPictureElement: null,
    head: new FakeElement("head", "head"),
    getElementById(id) {
      if (!elements.has(id)) elements.set(id, new FakeElement(id));
      return elements.get(id);
    },
    createElement(tag) { return new FakeElement("", tag); },
    createTextNode(text) { return text; },
    getElementsByTagName() { return []; },
    querySelectorAll(sel) {
      if (sel === ".result") return elements.get("results")?.children || [];
      if (sel === ".qitem") return elements.get("queueList")?.children || [];
      return [];
    },
    addEventListener(type, handler) {
      (documentHandlers[type] ??= []).push(handler);
    },
    emit(type, extra = {}) {
      for (const h of [...(documentHandlers[type] ?? [])]) {
        h({ target: documentObj, type, preventDefault() {}, ...extra });
      }
    },
    async exitPictureInPicture() {
      const prev = documentObj.pictureInPictureElement;
      documentObj.pictureInPictureElement = null;
      if (prev) prev.emit("leavepictureinpicture");
    },
  };

  const storageMap = new Map();
  const windowObj = {
    __TAURI__: {
      core: {
        invoke: (cmd, args) => (cmd === "home_section" ? homeInvoke(args) : invoke(cmd, args)),
      },
      event: {
        listen(name, cb) {
          (tauriListeners[name] ??= []).push(cb);
          return Promise.resolve(() => {});
        },
      },
    },
    addEventListener(type, handler) { (windowHandlers[type] ??= []).push(handler); },
    removeEventListener(type, handler) {
      if (!windowHandlers[type]) return;
      windowHandlers[type] = windowHandlers[type].filter((h) => h !== handler);
    },
    emit(type, extra = {}) {
      for (const h of [...(windowHandlers[type] ?? [])]) {
        h({ target: windowObj, type, ...extra });
      }
    },
  };

  const contextGlobals = {
    console,
    document: documentObj,
    window: windowObj,
    navigator: opts.navigator || {},
    localStorage: opts.localStorage || {
      getItem: (k) => (storageMap.has(k) ? storageMap.get(k) : null),
      setItem: (k, v) => storageMap.set(k, String(v)),
    },
    setInterval(fn, ms) {
      intervals.push({ fn, ms });
      context.sync = fn;
      return intervals.length;
    },
    clearInterval() {},
    setTimeout(fn, ms) {
      const id = nextTimerId++;
      timers.set(id, { fn, ms });
      return id;
    },
    clearTimeout(id) { timers.delete(id); },
  };
  if (opts.YT) contextGlobals.YT = opts.YT;

  const appSrc = fs.readFileSync(path.join(SRC_DIR, "app.js"), "utf8");
  const context = vm.createContext(contextGlobals);
  vm.runInContext(
    `${appSrc}\n;globalThis.__e2eApi = { state, audio, video, activeEl, pickVideo, pickAudio, getAudioCandidates, getVideoCandidates, playCurrent, togglePlay, seekTo, onEnded, onError, next, prev, toggleRepeat, toggleShuffle, setQueue, updateModeButtonsUI, goHome, doSearch, openCollection, loadHome, renderResults, renderHome, onTimeUpdate, recoverPlayback, scheduleStallRecovery, applyInitialPosition, failPlayback, STALL_TIMEOUT_MS, MAX_STREAM_REFRESHES };`,
    context
  );

  async function flushTimers() {
    const entries = [...timers.entries()];
    timers.clear();
    for (const [, entry] of entries) {
      await entry.fn();
    }
    return entries.length;
  }

  return {
    ...context.__e2eApi,
    syncIntervals: intervals,
    get: (id) => documentObj.getElementById(id),
    window: windowObj,
    document: documentObj,
    flushTimers,
    pendingTimers: () => [...timers.values()],
    emitTauri: (name, payload) => {
      for (const cb of tauriListeners[name] || []) cb({ event: name, payload });
    },
    appSrc,
  };
}

module.exports = {
  REPO_ROOT,
  SRC_DIR,
  createAppServer,
  createBrowserHarness,
  createDomVmHarness,
  parseRangeHeader,
};
