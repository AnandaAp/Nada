"use strict";

const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { SRC_DIR, createDomVmHarness } = require("./lib/harness.cjs");

const sampleStreams = {
  muxed: [{ url: "http://127.0.0.1:9000/s/m1", mime: 'video/mp4; codecs="avc1.4d401f"', quality: "720p", height: 720 }],
  video_only: [{ url: "http://127.0.0.1:9000/s/v1", mime: 'video/mp4; codecs="avc1.4d401f"', quality: "720p", height: 720 }],
  audio: [
    { url: "http://127.0.0.1:9000/s/a1", mime: 'audio/mp4; codecs="mp4a.40.2"', bitrate: 128000 },
    { url: "http://127.0.0.1:9000/s/a2", mime: 'audio/webm; codecs="opus"', bitrate: 160000 },
  ],
  expires_in_seconds: 21600,
};

// --- F4: Stall/Error Recovery Contracts ---

test("T1-F4-01: stalled listener is registered on audio and video and schedules recovery when readyState < 3", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd) => (cmd === "streams" ? sampleStreams : undefined),
  });
  h.state.queue = [{ id: "t1", name: "Kalah 2", artist: "Aftershine", video: false }];
  h.state.index = 0;
  await h.playCurrent();

  h.audio.readyState = 2;
  h.audio.currentTime = 64;
  h.audio.emit("stalled");
  assert.equal(h.state.buffering, true);
  assert.ok(h.state.recoveryTimer !== null, "audio stalled must schedule recovery timer");

  // Also in muxed video mode
  h.state.wantVideo = true;
  await h.playCurrent();
  assert.equal(h.state.mode, "muxed");
  h.video.readyState = 2;
  h.video.currentTime = 64;
  h.video.emit("stalled");
  assert.equal(h.state.buffering, true);
  assert.ok(h.state.recoveryTimer !== null, "video stalled must schedule recovery timer");
});

test("T1-F4-02: waiting listener covers audio, muxed, and split modes cleanly", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd) => (cmd === "streams" ? sampleStreams : undefined),
  });
  h.state.queue = [{ id: "t1", name: "Track 1", artist: "Art", video: false }];
  h.state.index = 0;
  await h.playCurrent();
  assert.equal(h.state.mode, "audio");
  h.audio.readyState = 2;
  h.audio.emit("waiting");
  assert.equal(h.state.buffering, true);
  assert.equal(h.get("status").classList.contains("hidden"), false);

  h.audio.readyState = 4;
  h.audio.emit("playing");
  assert.equal(h.state.buffering, false);

  // Split mode pauses partner on waiting
  const splitOnly = { muxed: [], video_only: sampleStreams.video_only, audio: sampleStreams.audio };
  const h2 = createDomVmHarness({
    invoke: async (cmd) => (cmd === "streams" ? splitOnly : undefined),
  });
  h2.state.queue = [{ id: "t2", name: "Track 2", artist: "Art", video: true }];
  h2.state.index = 0;
  h2.state.wantVideo = true;
  await h2.playCurrent();
  assert.equal(h2.state.mode, "split");
  h2.video.readyState = 2;
  h2.video.emit("waiting");
  assert.equal(h2.audio.paused, true);
  assert.equal(h2.video.paused, true);
  assert.equal(h2.state.splitBufferingPaused, true);
});

test("T1-F4-03: mid-stream error captures activeEl().currentTime before candidate retry", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd) => (cmd === "streams" ? sampleStreams : undefined),
    opts: { resetReadyStateOnSrc: true },
  });
  h.state.queue = [{ id: "kalah2", name: "Kalah 2", artist: "Aftershine", video: false }];
  h.state.index = 0;
  await h.playCurrent();
  h.audio.readyState = 4;
  h.audio.currentTime = 64.5;

  h.audio.emit("error");
  await new Promise((r) => setImmediate(r));
  assert.equal(h.state.pendingResumePos, 64.5);
  assert.equal(h.audio.src, sampleStreams.audio[1].url);
  h.audio.readyState = 1;
  h.audio.emit("loadedmetadata");
  assert.equal(h.audio.currentTime, 64.5);
});

test("T1-F4-04: getAudioCandidates prioritizes mp4 > webm > others and iterates in order", () => {
  const h = createDomVmHarness();
  const ordered = h.getAudioCandidates([
    { url: "o1", mime: "audio/ogg" },
    { url: "w1", mime: 'audio/webm; codecs="opus"' },
    { url: "bad", mime: 'audio/mp4; codecs="unsupported"' },
    { url: "m1", mime: 'audio/mp4; codecs="mp4a.40.2"' },
  ]);
  assert.deepEqual(
    Array.from(ordered, (x) => x.url),
    ["m1", "w1", "o1"]
  );
});

test("T1-F4-05: ytPlayer fallback is NOT silently invoked on mid-stream native error or stall", async () => {
  let ytLoadCalls = 0;
  class StubYTPlayer {
    constructor(_, cfg) {
      cfg?.events?.onReady?.();
    }
    loadVideoById() { ytLoadCalls++; }
    stopVideo() {}
    setVolume() {}
  }
  const h = createDomVmHarness({
    invoke: async (cmd) => {
      if (cmd === "streams") return { muxed: [], video_only: [], audio: [sampleStreams.audio[0]] };
    },
    opts: { YT: { Player: StubYTPlayer, PlayerState: { PLAYING: 1, PAUSED: 2, ENDED: 0, BUFFERING: 3 } } },
  });
  h.window.onYouTubeIframeAPIReady();
  h.state.queue = [{ id: "t1", name: "Song", artist: "A", video: false }];
  h.state.index = 0;
  await h.playCurrent();

  h.audio.currentTime = 64;
  h.audio.emit("error");
  await new Promise((r) => setImmediate(r));
  h.audio.emit("error");
  await new Promise((r) => setImmediate(r));
  h.audio.emit("error");
  await new Promise((r) => setImmediate(r));

  assert.equal(ytLoadCalls, 0);
  assert.equal(h.get("ytPlayer").classList.contains("hidden"), true);
});

// --- F5: Seek Restoration, Transitions & Concurrency Guards ---

test("T1-F5-01: applyInitialPosition waits for loadedmetadata when readyState is 0", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd) => (cmd === "streams" ? sampleStreams : undefined),
    opts: { resetReadyStateOnSrc: true },
  });
  h.state.queue = [{ id: "t1", name: "Song", artist: "A", video: false }];
  h.state.index = 0;

  await h.playCurrent(66, true);
  assert.equal(h.audio.readyState, 0);
  assert.equal(h.audio.currentTime, 0, "must not apply before metadata loads");
  h.audio.readyState = 1;
  h.audio.emit("loadedmetadata");
  assert.equal(h.audio.currentTime, 66);
});

test("T1-F5-02: next() radio in-flight lock prevents duplicate invoke('radio') calls", async () => {
  let radioCalls = 0;
  let resolveRadio;
  const h = createDomVmHarness({
    invoke: async (cmd) => {
      if (cmd === "streams") return sampleStreams;
      if (cmd === "radio") {
        radioCalls++;
        return new Promise((r) => { resolveRadio = r; });
      }
    },
  });
  h.state.queue = [{ id: "t1", name: "Song 1", artist: "A", video: false }];
  h.state.index = 0;
  await h.playCurrent();

  const p1 = h.next(true);
  const p2 = h.next(false);
  assert.equal(radioCalls, 1);
  resolveRadio([{ id: "t2", name: "Song 2", artist: "B", video: false }]);
  await Promise.all([p1, p2]);
  assert.equal(h.state.queue.length, 2);
});

test("T1-F5-03: ytPlayer callbacks are ignored whenever state.mode !== 'yt'", () => {
  let events;
  class StubYTPlayer {
    constructor(_, cfg) { events = cfg.events; }
    stopVideo() {}
    setVolume() {}
  }
  const h = createDomVmHarness({
    opts: {
      YT: { Player: StubYTPlayer, PlayerState: { PLAYING: 1, PAUSED: 2, ENDED: 0, BUFFERING: 3 } },
    },
  });
  h.window.onYouTubeIframeAPIReady();
  h.state.queue = [{ id: "t1", name: "1" }, { id: "t2", name: "2" }];
  h.state.index = 0;
  h.state.mode = "audio";
  h.state.wantPlaying = true;

  events.onStateChange({ data: 2 });
  events.onStateChange({ data: 0 });
  events.onError({ data: 101 });
  assert.equal(h.state.index, 0);
  assert.equal(h.state.wantPlaying, true);
});

test("T1-F5-04: state.seeking resets on cancel/blur/seeked even when duration is 0", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd) => (cmd === "streams" ? sampleStreams : undefined),
  });
  h.state.queue = [{ id: "t1", name: "Song", artist: "A", video: false }];
  h.state.index = 0;
  await h.playCurrent();
  h.audio.duration = 0;

  const progress = h.get("progress");
  progress.emit("input");
  assert.equal(h.state.seeking, true);
  progress.emit("change");
  assert.equal(h.state.seeking, false);

  progress.emit("input");
  assert.equal(h.state.seeking, true);
  h.window.emit("blur");
  assert.equal(h.state.seeking, false);
});

test("T1-F5-05: monotonic playbackId and viewId guards reject stale async responses", async () => {
  let resolveSearch, resolveStreams;
  const h = createDomVmHarness({
    invoke: async (cmd, args) => {
      if (cmd === "search") return new Promise((r) => { resolveSearch = r; });
      if (cmd === "streams" && args.videoId === "old") {
        return new Promise((r) => { resolveStreams = r; });
      }
      if (cmd === "streams") return sampleStreams;
    },
  });

  h.get("search").value = "kalah";
  const pendingSearch = h.doSearch();
  h.goHome();
  resolveSearch([{ id: "stale", name: "Stale" }]);
  await pendingSearch;
  assert.equal(h.state.view, "home");
  assert.equal(h.state.results.length, 0);

  h.state.queue = [{ id: "old", name: "Old" }, { id: "new", name: "New" }];
  h.state.index = 0;
  const pOld = h.playCurrent();
  h.state.index = 1;
  await h.playCurrent();
  resolveStreams({ muxed: [], video_only: [], audio: [{ url: "http://127.0.0.1/stale", mime: "audio/mp4" }] });
  await pOld;
  assert.equal(h.audio.src, sampleStreams.audio[0].url);
});

// --- F6: 16:9 & PiP CSS / DOM Contracts ---

test("T1-F6-01: all CSS custom properties var(--*) in src/styles.css resolve to :root definitions", () => {
  const css = fs.readFileSync(path.join(SRC_DIR, "styles.css"), "utf8");
  const lines = css.split(/\r?\n/);
  const rootStart = lines.findIndex((l) => l.trim().startsWith(":root"));
  const rootEnd = lines.findIndex((l, i) => i > rootStart && l.trim() === "}");
  assert.ok(rootStart !== -1 && rootEnd > rootStart);

  const defined = new Set();
  for (let i = rootStart; i <= rootEnd; i++) {
    const m = lines[i].match(/^\s*(--[a-zA-Z0-9_-]+)\s*:/);
    if (m) defined.add(m[1]);
  }
  const missing = [];
  lines.forEach((l, idx) => {
    for (const m of l.matchAll(/var\(\s*(--[a-zA-Z0-9_-]+)/g)) {
      if (!defined.has(m[1])) missing.push(`line ${idx + 1}: ${m[1]}`);
    }
  });
  assert.deepEqual(missing, [], `Undefined CSS vars found: ${missing.join(", ")}`);
});

test("T1-F6-02: .stage and .stage video declare 16 / 9 aspect-ratio", () => {
  const css = fs.readFileSync(path.join(SRC_DIR, "styles.css"), "utf8");
  assert.match(css, /\.stage\s*\{[^}]*aspect-ratio:\s*16\s*\/\s*9/s);
  assert.match(css, /\.stage\s+video\s*\{[^}]*aspect-ratio:\s*16\s*\/\s*9/s);
});

test("T1-F6-03: .stage video declares object-fit: contain (no crop)", () => {
  const css = fs.readFileSync(path.join(SRC_DIR, "styles.css"), "utf8");
  assert.match(css, /\.stage\s+video\s*\{[^}]*object-fit:\s*contain/s);
  assert.doesNotMatch(css, /\.stage\s+video\s*\{[^}]*object-fit:\s*cover/s);
});

test("T1-F6-04: #video enables playsinline & Picture-in-Picture with zero contextmenu suppression", () => {
  const html = fs.readFileSync(path.join(SRC_DIR, "index.html"), "utf8");
  const appJs = fs.readFileSync(path.join(SRC_DIR, "app.js"), "utf8");
  assert.match(html, /<video[^>]*id="video"[^>]*playsinline/i);
  assert.doesNotMatch(html, /disablepictureinpicture/i);
  assert.doesNotMatch(appJs, /contextmenu/i);
});

test("T1-F6-05: #pipBtn click stops propagation so video play/pause is unaffected", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd) => (cmd === "streams" ? sampleStreams : undefined),
  });
  h.state.queue = [{ id: "v1", name: "Vid", artist: "Art", video: true }];
  h.state.index = 0;
  h.state.wantVideo = true;
  await h.playCurrent();
  assert.equal(h.video.paused, false);

  let stageClicks = 0;
  h.get("videoStage").addEventListener("click", () => { stageClicks++; });
  let stopPropCalled = false;
  h.get("pipBtn").emit("click", {
    stopPropagation() { stopPropCalled = true; },
  });
  await Promise.resolve();
  assert.equal(stopPropCalled, true);
  assert.equal(stageClicks, 0);
  assert.equal(h.video.paused, false);
  assert.equal(h.document.pictureInPictureElement, h.video);
});

test("T1-F6-06: src/index.html contains all required player, video, PiP, and recovery DOM landmarks", () => {
  const html = fs.readFileSync(path.join(SRC_DIR, "index.html"), "utf8");
  const requiredIds = [
    "homeBtn", "refreshHome", "home", "homeSections", "collectionHead", "backHome",
    "search", "clearSearch", "searchBtn", "tabTracks", "tabVideos", "results",
    "empty", "status", "queuePanel", "queueList", "queueClose",
    "videoStage", "ytPlayer", "video", "pipBtn", "audio",
    "curTime", "progress", "durTime", "pCover", "pTitle", "pArtist",
    "prevBtn", "playBtn", "iconPlay", "iconPause", "nextBtn",
    "repeatBtn", "repeatBadge", "shuffleBtn", "videoBtn", "volume", "queueBtn", "toast",
  ];
  for (const id of requiredIds) {
    assert.ok(new RegExp(`id="${id}"`).test(html), `Missing required DOM id #${id}`);
  }
});
