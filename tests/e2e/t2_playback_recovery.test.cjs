"use strict";

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { createDomVmHarness } = require("./lib/harness.cjs");

const track1 = { id: "t1", name: "Kalah 2", artist: "Aftershine", video: false };
const track2 = { id: "t2", name: "Mangu", artist: "Fourtwnty", video: false };
const track3 = { id: "t3", name: "Track 3", artist: "Artist 3", video: false };

// --- F4: Frontend Recovery & Resume (6 cases) ---

test("T2-F4-01: mid-stream stalled at 64s clears when progress resumes or recovers at 64s on timeout", async () => {
  let streamCalls = 0;
  const h = createDomVmHarness({
    invoke: async (cmd) => {
      if (cmd !== "streams") return;
      streamCalls++;
      return {
        muxed: [],
        video_only: [],
        audio: [{ url: `http://127.0.0.1:9000/s/a-${streamCalls}`, mime: "audio/mp4" }],
        expires_in_seconds: 21600,
      };
    },
    opts: { resetReadyStateOnSrc: true },
  });
  h.state.queue = [track1];
  h.state.index = 0;
  await h.playCurrent();
  h.audio.readyState = 4;
  h.audio.currentTime = 64.0;

  // Case A: stalled resolves naturally before STALL_TIMEOUT_MS
  h.audio.readyState = 2;
  h.audio.emit("stalled");
  assert.equal(h.state.buffering, true);
  h.audio.readyState = 4;
  h.audio.currentTime = 65.0;
  h.audio.emit("timeupdate");
  assert.equal(h.state.buffering, false);
  assert.equal(h.state.recoveryTimer, null);
  assert.equal(streamCalls, 1);

  // Case B: stalled stays stuck -> stall timer triggers recovery at preserved 65.0s
  h.audio.readyState = 2;
  h.audio.emit("stalled");
  assert.ok(h.state.recoveryTimer !== null);
  await h.flushTimers();
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 2);
  assert.equal(h.audio.src, "http://127.0.0.1:9000/s/a-2");
  h.audio.readyState = 1;
  h.audio.emit("loadedmetadata");
  assert.equal(h.audio.currentTime, 65.0);
});

test("T2-F4-02: mid-stream error at 64s switches to remaining candidate at preserved currentTime", async () => {
  let streamCalls = 0;
  const h = createDomVmHarness({
    invoke: async (cmd) => {
      if (cmd !== "streams") return;
      streamCalls++;
      return {
        muxed: [],
        video_only: [],
        audio: [
          { url: "http://127.0.0.1:9000/s/cand1", mime: 'audio/mp4; codecs="mp4a.40.2"' },
          { url: "http://127.0.0.1:9000/s/cand2", mime: 'audio/webm; codecs="opus"' },
        ],
        expires_in_seconds: 21600,
      };
    },
    opts: { resetReadyStateOnSrc: true },
  });
  h.state.queue = [track1];
  h.state.index = 0;
  await h.playCurrent();
  h.audio.readyState = 4;
  h.audio.currentTime = 64.4;

  h.audio.emit("error");
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 1);
  assert.equal(h.audio.src, "http://127.0.0.1:9000/s/cand2");
  h.audio.readyState = 1;
  h.audio.emit("loadedmetadata");
  assert.equal(h.audio.currentTime, 64.4);
  assert.equal(h.audio.paused, false);
});

test("T2-F4-03: exhausting candidates refreshes streams for fresh proxy URLs and resumes at 64s", async () => {
  let streamCalls = 0;
  const h = createDomVmHarness({
    invoke: async (cmd) => {
      if (cmd !== "streams") return;
      streamCalls++;
      return {
        muxed: [{ url: `http://127.0.0.1:9000/s/m-${streamCalls}`, mime: 'video/mp4; codecs="avc1.4d401f"', height: 720 }],
        video_only: [],
        audio: [],
        expires_in_seconds: 21600,
      };
    },
    opts: { resetReadyStateOnSrc: true },
  });
  h.state.queue = [{ ...track1, video: true }];
  h.state.index = 0;
  h.state.wantVideo = true;
  await h.playCurrent();
  assert.equal(h.state.mode, "muxed");
  h.video.readyState = 4;
  h.video.currentTime = 64.8;

  h.video.emit("error");
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 2);
  assert.equal(h.video.src, "http://127.0.0.1:9000/s/m-2");
  h.video.readyState = 1;
  h.video.emit("loadedmetadata");
  assert.equal(h.video.currentTime, 64.8);
  assert.equal(h.get("ytPlayer").classList.contains("hidden"), true);
});

test("T2-F4-04: waiting buffers cleanly and clears on canplay in audio, muxed, and split modes", async () => {
  const splitPayload = {
    muxed: [],
    video_only: [{ url: "http://127.0.0.1:9000/s/v1", mime: "video/mp4", height: 720 }],
    audio: [{ url: "http://127.0.0.1:9000/s/a1", mime: "audio/mp4" }],
  };
  const h = createDomVmHarness({
    invoke: async (cmd) => (cmd === "streams" ? splitPayload : undefined),
  });

  // Audio mode waiting -> canplay
  h.state.queue = [track1];
  h.state.index = 0;
  h.state.wantVideo = false;
  await h.playCurrent();
  h.audio.readyState = 2;
  h.audio.emit("waiting");
  assert.equal(h.state.buffering, true);
  assert.equal(h.get("status").classList.contains("hidden"), false);
  h.audio.readyState = 4;
  h.audio.emit("canplay");
  assert.equal(h.state.buffering, false);
  assert.equal(h.get("status").classList.contains("hidden"), true);

  // Split mode waiting pauses both without flashing play icon, resumes on canplay
  h.state.wantVideo = true;
  await h.playCurrent();
  assert.equal(h.state.mode, "split");
  h.video.readyState = 2;
  h.video.emit("waiting");
  assert.equal(h.audio.paused, true);
  assert.equal(h.video.paused, true);
  assert.equal(h.get("iconPlay").classList.contains("hidden"), true);
  assert.equal(h.get("iconPause").classList.contains("hidden"), false);
  h.video.readyState = 4;
  h.video.emit("canplay");
  await Promise.resolve();
  assert.equal(h.audio.paused, false);
  assert.equal(h.video.paused, false);
  assert.equal(h.state.buffering, false);
});

test("T2-F4-05: AbortError during play or recovery does not trigger failPlayback", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd) =>
      cmd === "streams"
        ? { muxed: [], video_only: [], audio: [{ url: "http://127.0.0.1:9000/s/a", mime: "audio/mp4" }] }
        : undefined,
  });
  h.state.queue = [track1];
  h.state.index = 0;
  await h.playCurrent();
  await h.togglePlay(); // pause
  h.audio.playError = Object.assign(new Error("aborted"), { name: "AbortError" });
  await h.togglePlay(); // interrupted play
  assert.equal(h.state.mode, "audio");
  assert.equal(h.audio.src, "http://127.0.0.1:9000/s/a");
  assert.equal(h.get("toast").classList.contains("hidden"), true);
});

test("T2-F4-06: terminal exhaustion cleans UI state cleanly and allows subsequent track recovery", async () => {
  let streamCalls = 0;
  let shouldFail = true;
  const h = createDomVmHarness({
    invoke: async (cmd, args) => {
      if (cmd !== "streams") return;
      streamCalls++;
      if (shouldFail) throw new Error("upstream 502");
      return {
        muxed: [],
        video_only: [],
        audio: [{ url: `http://127.0.0.1:9000/s/${args.videoId}`, mime: "audio/mp4" }],
      };
    },
  });
  h.state.queue = [track1, track2];
  h.state.index = 0;
  await h.playCurrent();
  assert.equal(h.state.mode, null);
  assert.equal(h.state.wantPlaying, false);
  assert.equal(h.get("iconPlay").classList.contains("hidden"), false);
  assert.equal(h.get("iconPause").classList.contains("hidden"), true);
  assert.equal(h.get("toast").classList.contains("hidden"), false);

  // Subsequent track plays cleanly
  shouldFail = false;
  h.state.index = 1;
  await h.playCurrent();
  assert.equal(h.state.mode, "audio");
  assert.equal(h.audio.paused, false);
  assert.equal(h.audio.src, "http://127.0.0.1:9000/s/t2");
  assert.ok(streamCalls >= 2);
});

// --- F5: Seek, Transitions & Queue (6 cases) ---

test("T2-F5-01: initial seek playCurrent(65, true) applies 65s on loadedmetadata in audio, muxed, split", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd, args) => {
      if (cmd !== "streams") return;
      if (args.videoId === "muxed") {
        return { muxed: [{ url: "http://127.0.0.1/m", mime: "video/mp4", height: 720 }], video_only: [], audio: [] };
      }
      if (args.videoId === "split") {
        return { muxed: [], video_only: [{ url: "http://127.0.0.1/v", mime: "video/mp4", height: 720 }], audio: [{ url: "http://127.0.0.1/a", mime: "audio/mp4" }] };
      }
      return { muxed: [], video_only: [], audio: [{ url: "http://127.0.0.1/ao", mime: "audio/mp4" }] };
    },
    opts: { resetReadyStateOnSrc: true },
  });
  h.state.queue = [
    { id: "aud", name: "Aud", video: false },
    { id: "muxed", name: "Mux", video: true },
    { id: "split", name: "Spl", video: true },
  ];

  // Audio
  h.state.index = 0;
  h.state.wantVideo = false;
  await h.playCurrent(65, true);
  h.audio.readyState = 1;
  h.audio.emit("loadedmetadata");
  assert.equal(h.audio.currentTime, 65);

  // Muxed
  h.state.index = 1;
  h.state.wantVideo = true;
  await h.playCurrent(65, true);
  h.video.readyState = 1;
  h.video.emit("loadedmetadata");
  assert.equal(h.video.currentTime, 65);

  // Split
  h.state.index = 2;
  h.state.wantVideo = true;
  await h.playCurrent(65, true);
  h.audio.readyState = 1;
  h.video.readyState = 1;
  h.audio.emit("loadedmetadata");
  h.video.emit("loadedmetadata");
  assert.equal(h.audio.currentTime, 65);
  assert.equal(h.video.currentTime, 65);
});

test("T2-F5-02: rapid double next() at queue end fires single radio and deduplicates queue", async () => {
  let radioCalls = 0;
  let resolveRadio;
  const h = createDomVmHarness({
    invoke: async (cmd) => {
      if (cmd === "streams") {
        return { muxed: [], video_only: [], audio: [{ url: "http://127.0.0.1/a", mime: "audio/mp4" }] };
      }
      if (cmd === "radio") {
        radioCalls++;
        return new Promise((r) => { resolveRadio = r; });
      }
    },
  });
  h.state.queue = [track1];
  h.state.index = 0;
  await h.playCurrent();

  const p1 = h.next(true);
  const p2 = h.next(true);
  assert.equal(radioCalls, 1);
  resolveRadio([track1, track2, track3]); // includes duplicate track1
  await Promise.all([p1, p2]);
  assert.deepEqual(h.state.queue.map((t) => t.id), ["t1", "t2", "t3"]);
  assert.equal(h.state.index, 1);
});

test("T2-F5-03: prev() rewinds when currentTime > 3 and navigates back when <= 3", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd, args) =>
      cmd === "streams"
        ? { muxed: [], video_only: [], audio: [{ url: `http://127.0.0.1/${args.videoId}`, mime: "audio/mp4" }] }
        : undefined,
  });
  h.state.queue = [track1, track2, track3];
  h.state.index = 1;
  await h.playCurrent();

  h.audio.currentTime = 12;
  h.prev();
  assert.equal(h.state.index, 1);
  assert.equal(h.audio.currentTime, 0);

  h.audio.currentTime = 2.5;
  h.prev();
  await new Promise((r) => setImmediate(r));
  assert.equal(h.state.index, 0);
  assert.equal(h.audio.src, "http://127.0.0.1/t1");

  // Wrap around when repeat === "all"
  h.state.repeat = "all";
  h.audio.currentTime = 1.0;
  h.prev();
  await new Promise((r) => setImmediate(r));
  assert.equal(h.state.index, 2);
  assert.equal(h.audio.src, "http://127.0.0.1/t3");
});

test("T2-F5-04: repeat off/all/one across auto-ended vs manual next() and UI badge", async () => {
  const h = createDomVmHarness({
    invoke: async (cmd) =>
      cmd === "streams"
        ? { muxed: [], video_only: [], audio: [{ url: "http://127.0.0.1/a", mime: "audio/mp4" }] }
        : undefined,
  });
  h.state.queue = [track1, track2];
  h.state.index = 0;
  await h.playCurrent();

  h.toggleRepeat(); // off -> all
  assert.equal(h.state.repeat, "all");
  assert.equal(h.get("repeatBtn").classList.contains("active"), true);
  assert.equal(h.get("repeatBadge").classList.contains("hidden"), true);

  h.toggleRepeat(); // all -> one
  assert.equal(h.state.repeat, "one");
  assert.equal(h.get("repeatBadge").classList.contains("hidden"), false);

  // Auto ended stays on index 0
  h.audio.emit("ended");
  await new Promise((r) => setImmediate(r));
  assert.equal(h.state.index, 0);

  // Manual next advances even in repeat one
  await h.next(false);
  assert.equal(h.state.index, 1);

  // Repeat all wraps on auto ended
  h.toggleRepeat(); // one -> off
  h.toggleRepeat(); // off -> all
  h.audio.emit("ended");
  await new Promise((r) => setImmediate(r));
  assert.equal(h.state.index, 0);
});

test("T2-F5-05: toggleShuffle preserves current track and restores originalQueue order when off", () => {
  const h = createDomVmHarness();
  const items = [track1, track2, track3, { id: "t4", name: "Track 4" }];
  h.setQueue(items, 1); // track2
  assert.equal(h.state.queue[h.state.index].id, "t2");

  h.toggleShuffle();
  assert.equal(h.state.shuffle, true);
  assert.equal(h.get("shuffleBtn").classList.contains("active"), true);
  assert.equal(h.state.queue[h.state.index].id, "t2");
  assert.equal(h.state.queue.length, 4);

  h.toggleShuffle();
  assert.equal(h.state.shuffle, false);
  assert.equal(h.get("shuffleBtn").classList.contains("active"), false);
  assert.equal(h.state.queue.map((t) => t.id).join(","), "t1,t2,t3,t4");
  assert.equal(h.state.index, 1);
});

test("T2-F5-06: wantVideo persists across transitions, videoBtn preserves pos, audio search clears wantVideo & exits PiP", async () => {
  const streams = {
    muxed: [],
    video_only: [{ url: "http://127.0.0.1/v", mime: "video/mp4", height: 720 }],
    audio: [{ url: "http://127.0.0.1/a", mime: "audio/mp4" }],
  };
  const h = createDomVmHarness({
    invoke: async (cmd) => (cmd === "streams" ? streams : undefined),
    opts: { resetReadyStateOnSrc: true },
  });
  h.state.queue = [{ ...track1, video: true }, { ...track2, video: false }];
  h.state.index = 0;
  h.state.wantVideo = true;
  await h.playCurrent();
  h.audio.readyState = 4;
  h.video.readyState = 4;
  assert.equal(h.state.mode, "split");

  // Next track preserves wantVideo
  await h.next(false);
  assert.equal(h.state.wantVideo, true);
  assert.equal(h.state.mode, "split");

  // Toggle video off at 68s while paused -> audio resumes paused at 68s after loadedmetadata
  h.audio.readyState = 4;
  h.video.readyState = 4;
  h.seekTo(68);
  await h.togglePlay(); // pause
  h.get("videoBtn").emit("click");
  await new Promise((r) => setImmediate(r));
  assert.equal(h.state.wantVideo, false);
  assert.equal(h.state.mode, "audio");
  h.audio.readyState = 1;
  h.audio.emit("loadedmetadata");
  assert.equal(h.audio.currentTime, 68);
  assert.equal(h.audio.paused, true);

  // Turn video back on, enter PiP, then click an audio search result -> clears wantVideo & exits PiP
  h.get("videoBtn").emit("click");
  await new Promise((r) => setImmediate(r));
  h.audio.readyState = 4;
  h.video.readyState = 4;
  await h.video.requestPictureInPicture();
  assert.equal(h.document.pictureInPictureElement, h.video);

  h.state.view = "search";
  h.state.results = [{ id: "aud1", name: "Audio Track", artist: "Art", video: false }];
  h.renderResults();
  h.get("results").children[0].click();
  await new Promise((r) => setImmediate(r));
  assert.equal(h.state.wantVideo, false);
  assert.equal(h.state.mode, "audio");
  assert.equal(h.document.pictureInPictureElement, null);
});
