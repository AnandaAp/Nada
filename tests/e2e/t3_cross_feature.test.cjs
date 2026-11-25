"use strict";

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { createBrowserHarness } = require("./lib/harness.cjs");

test("T3-03 (F2 x F4): mid-stream 502/drop and stall timeout at ~64s preserve position, refresh streams, and play past 1:04 without ytPlayer", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640, stubYT: true });
  try {
    const f = h.fixtures.audioMain;
    const cutoffDrop = f.byteOffsetAtSec(63);
    const cutoffStall = f.byteOffsetAtSec(62.1);
    const badUrl = h.registerTrackMedia("t303_v1", f, {
      dropAfterBytes: cutoffDrop,
      errorStatus: 502,
      errorAfterBytes: cutoffDrop,
    });
    const stallUrl = h.registerTrackMedia("t303_v2_stall", f, {
      stallAfterBytes: cutoffStall,
      stallMs: 9000,
    });
    const goodUrl = h.registerTrackMedia("t303_v3", f);

    let streamCall = 0;
    h.appServer.setIpcHandler("search", async () => [
      { id: "kalah2", name: "Kalah 2", artist: "Aftershine", duration: 80, video: false },
    ]);
    h.appServer.setIpcHandler("streams", async () => {
      streamCall++;
      const url = streamCall === 1 ? badUrl : streamCall === 2 ? stallUrl : goodUrl;
      return {
        muxed: [],
        video_only: [],
        audio: [{ url, mime: f.mime, bitrate: 176400 }],
        expires_in_seconds: 21600,
      };
    });

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Kalah 2";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");

    // Wait for initial v1 playback, then seek to 61.5s so playback crosses the 63s drop+502
    await h.edge.waitFor(() => document.getElementById("audio").readyState >= 2);
    await h.edge.evaluate(() => {
      document.getElementById("audio").currentTime = 61.5;
    });

    // Phase 1: Drop + 502 recovers onto v2_stall; Phase 2: v2_stall stalls > STALL_TIMEOUT_MS (7s)
    // at 64s and scheduleStallRecovery refreshes streams onto t303_v3 past 1:04
    await h.edge.waitFor(
      () => {
        const a = document.getElementById("audio");
        return a.src.includes("t303_v3") && a.currentTime >= 65.5 && !a.paused;
      },
      { timeoutMs: 25000, label: "drop+stall recovery onto t303_v3 past 65.5s" }
    );

    const state = await h.edge.evaluate(() => ({
      src: document.getElementById("audio").src,
      t: document.getElementById("audio").currentTime,
      curTime: document.getElementById("curTime").textContent,
      ytHidden: document.getElementById("ytPlayer").classList.contains("hidden"),
      ytLoads: window.__NADA_YT_LOADS__.length,
    }));
    assert.ok(streamCall >= 3, `Expected stream refresh >= 3 (drop + stall), got ${streamCall}`);
    assert.ok(state.t >= 65.5, `Expected preserved recovery past 1:04, got ${state.t}`);
    assert.equal(state.ytHidden, true);
    assert.equal(state.ytLoads, 0, "ytPlayer fallback must never fire during drop/stall recovery");
  } finally {
    await h.close();
  }
});

test("T3-04 (F3 x F4): startup 404 and post-startup evicted-token 404 trigger fresh streams IPC and recover", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const f = h.fixtures.audioMain;
    const evictedStartupUrl = `${h.baseUrl}/media/missing_evicted_startup`;
    const cutoffBytes = f.byteOffsetAtSec(64);
    const evictedMidUrl = h.registerTrackMedia("t304_evict_mid", f, {
      dropAfterBytes: cutoffBytes,
      errorStatus: 404,
      errorAfterBytes: cutoffBytes,
    });
    const freshUrl = h.registerTrackMedia("t304_fresh", f);

    let streamCall = 0;
    h.appServer.setIpcHandler("search", async () => [
      { id: "kalah2", name: "Kalah 2", artist: "Aftershine", duration: 80, video: false },
    ]);
    h.appServer.setIpcHandler("streams", async () => {
      streamCall++;
      const url = streamCall === 1 ? evictedStartupUrl : streamCall === 2 ? evictedMidUrl : freshUrl;
      return {
        muxed: [],
        video_only: [],
        audio: [{ url, mime: f.mime, bitrate: 176400 }],
        expires_in_seconds: 21600,
      };
    });

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Kalah 2";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");

    // Phase 1: Startup 404 refreshes streams onto t304_evict_mid
    await h.edge.waitFor(
      () => {
        const a = document.getElementById("audio");
        return a.src.includes("t304_evict_mid") && !a.paused && a.readyState >= 3;
      },
      { timeoutMs: 12000, label: "recovered from startup 404 onto t304_evict_mid" }
    );

    // Phase 2: Seek to 62s -> post-startup evicted-token 404 at 64s -> fresh streams onto t304_fresh past 1:04
    await h.edge.evaluate(() => {
      document.getElementById("audio").currentTime = 62;
    });
    await h.edge.waitFor(
      () => {
        const a = document.getElementById("audio");
        return a.src.includes("t304_fresh") && a.currentTime >= 65.5 && !a.paused && a.readyState >= 3;
      },
      { timeoutMs: 12000, label: "recovered from post-startup 404 onto t304_fresh" }
    );
    assert.ok(streamCall >= 3, `Expected >= 3 streams calls (startup 404 + mid-stream 404), got ${streamCall}`);
  } finally {
    await h.close();
  }
});

test("T3-05 (F4 x F5): recovery during next() respects playbackId so slow Track 1 cannot hijack Track 2", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const f = h.fixtures.audioMain;
    const t1Bad = `${h.baseUrl}/media/t305_missing`;
    const t1Stale = h.registerTrackMedia("t305_stale", f);
    const t2Good = h.registerTrackMedia("t305_track2", h.fixtures.audioAlt);

    let t1Calls = 0;
    h.appServer.setIpcHandler("search", async () => [
      { id: "t1", name: "Track One", artist: "A", duration: 80, video: false },
      { id: "t2", name: "Track Two", artist: "B", duration: 78, video: false },
    ]);
    h.appServer.setIpcHandler("streams", async ({ videoId }) => {
      if (videoId === "t1") {
        t1Calls++;
        if (t1Calls === 1) {
          return { muxed: [], video_only: [], audio: [{ url: t1Bad, mime: f.mime, bitrate: 176400 }], expires_in_seconds: 21600 };
        }
        // Delay stale recovery refresh for 700ms
        await new Promise((r) => setTimeout(r, 700));
        return { muxed: [], video_only: [], audio: [{ url: t1Stale, mime: f.mime, bitrate: 176400 }], expires_in_seconds: 21600 };
      }
      return { muxed: [], video_only: [], audio: [{ url: t2Good, mime: h.fixtures.audioAlt.mime, bitrate: 176400 }], expires_in_seconds: 21600 };
    });

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Track";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 2);
    await h.edge.clickSelector('.result[data-id="t1"]');
    // Immediately click next while t1 recovery is pending
    await new Promise((r) => setTimeout(r, 120));
    await h.edge.clickSelector("#nextBtn");

    await h.edge.waitFor(
      () => document.getElementById("audio").src.includes("t305_track2") && !document.getElementById("audio").paused,
      { timeoutMs: 8000, label: "Track 2 active" }
    );
    // Wait past the 700ms stale t1 resolution window and verify t2 is untouched
    await new Promise((r) => setTimeout(r, 900));
    const finalState = await h.edge.evaluate(() => ({
      src: document.getElementById("audio").src,
      title: document.getElementById("pTitle").textContent,
    }));
    assert.ok(finalState.src.includes("t305_track2"), `Stale Track 1 hijacked src: ${finalState.src}`);
    assert.equal(finalState.title, "Track Two");
  } finally {
    await h.close();
  }
});

test("T3-06 (F5 x F6): toggling videoBtn on/off at >65s restores position after loadedmetadata and preserves 16:9 & PiP", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const vUrl = h.registerTrackMedia("t306_v", h.fixtures.video16x9);
    const aUrl = h.registerTrackMedia("t306_a", h.fixtures.audioMain);
    h.appServer.setIpcHandler("search", async () => [
      { id: "kalah2", name: "Kalah 2", artist: "Aftershine", duration: 80, video: false },
    ]);
    h.appServer.setIpcHandler("streams", async () => ({
      muxed: [],
      video_only: [{ url: vUrl, mime: h.fixtures.video16x9.mime, quality: "360p", height: 180 }],
      audio: [{ url: aUrl, mime: h.fixtures.audioMain.mime, bitrate: 176400 }],
      expires_in_seconds: 21600,
    }));

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Kalah 2";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");
    await h.edge.waitFor(() => document.getElementById("audio").readyState >= 3);

    // Seek to 65s in audio mode, then toggle videoBtn ON
    await h.edge.evaluate(() => { document.getElementById("audio").currentTime = 65; });
    await h.edge.waitFor(() => document.getElementById("audio").currentTime >= 65);
    await h.edge.clickSelector("#videoBtn");

    await h.edge.waitFor(
      () => {
        const v = document.getElementById("video");
        const a = document.getElementById("audio");
        return !document.getElementById("videoStage").classList.contains("hidden") &&
          v.readyState >= 3 && a.readyState >= 3 && a.currentTime >= 64.5;
      },
      { timeoutMs: 10000, label: "video mode restored at >=64.5s" }
    );

    // Enter PiP, then toggle videoBtn OFF -> must exit PiP and preserve >=64s position in audio
    await h.edge.clickSelector("#pipBtn");
    await h.edge.waitFor(() => document.pictureInPictureElement === document.getElementById("video"));
    await h.edge.clickSelector("#videoBtn");
    await h.edge.waitFor(
      () =>
        document.pictureInPictureElement === null &&
        document.getElementById("videoStage").classList.contains("hidden") &&
        document.getElementById("audio").readyState >= 3 &&
        document.getElementById("audio").currentTime >= 64.5,
      { timeoutMs: 10000, label: "audio mode restored after video off" }
    );
  } finally {
    await h.close();
  }
});

test("T3-07 (F4 x F6): native video error with YT global present refreshes streams instead of switching to #ytPlayer", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640, stubYT: true });
  try {
    const vBad = `${h.baseUrl}/media/t307_bad`;
    const vGood = h.registerTrackMedia("t307_vgood", h.fixtures.video16x9);
    const aGood = h.registerTrackMedia("t307_agood", h.fixtures.audioMain);

    let calls = 0;
    h.appServer.setIpcHandler("search", async () => [
      { id: "vid1", name: "Video 1", artist: "Art", duration: 80, video: true },
    ]);
    h.appServer.setIpcHandler("streams", async () => {
      calls++;
      return {
        muxed: [],
        video_only: [{ url: calls === 1 ? vBad : vGood, mime: h.fixtures.video16x9.mime, quality: "360p", height: 180 }],
        audio: [{ url: aGood, mime: h.fixtures.audioMain.mime, bitrate: 176400 }],
        expires_in_seconds: 21600,
      };
    });

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Video 1";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");

    await h.edge.waitFor(
      () => {
        const v = document.getElementById("video");
        return v.src.includes("t307_vgood") && v.videoWidth === 320 && !v.paused;
      },
      { timeoutMs: 12000, label: "native video recovered without ytPlayer" }
    );
    const ytState = await h.edge.evaluate(() => ({
      ytLoads: window.__NADA_YT_LOADS__.length,
      ytHidden: document.getElementById("ytPlayer").classList.contains("hidden"),
      objectFit: getComputedStyle(document.getElementById("video")).objectFit,
    }));
    assert.equal(ytState.ytLoads, 0);
    assert.equal(ytState.ytHidden, true);
    assert.equal(ytState.objectFit, "contain");
  } finally {
    await h.close();
  }
});

test("T3-08 (F5 x F7): 430x520 min height with queue drawer open + split playback + seek keeps controls accessible", async () => {
  const h = await createBrowserHarness({ width: 430, height: 520 });
  try {
    const vUrl = h.registerTrackMedia("t308_v", h.fixtures.video16x9);
    const aUrl = h.registerTrackMedia("t308_a", h.fixtures.audioMain);
    h.appServer.setIpcHandler("search", async () => [
      { id: "v1", name: "Vid 1", artist: "Art", duration: 80, video: true },
      { id: "v2", name: "Vid 2", artist: "Art", duration: 80, video: true },
    ]);
    h.appServer.setIpcHandler("streams", async () => ({
      muxed: [],
      video_only: [{ url: vUrl, mime: h.fixtures.video16x9.mime, quality: "360p", height: 180 }],
      audio: [{ url: aUrl, mime: h.fixtures.audioMain.mime, bitrate: 176400 }],
      expires_in_seconds: 21600,
    }));

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Vid";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 2);
    await h.edge.clickSelector('.result[data-id="v1"]');
    await h.edge.waitFor(() => document.getElementById("video").readyState >= 3);

    await h.edge.clickSelector("#queueBtn");
    await h.edge.waitFor(() => !document.getElementById("queuePanel").classList.contains("hidden"));

    // Seek via ArrowRight / progress and verify dock remains non-clipped inside 520px
    await h.edge.evaluate(() => {
      document.getElementById("audio").currentTime = 66;
      document.getElementById("video").currentTime = 66;
    });
    await h.edge.waitFor(() => document.getElementById("audio").currentTime >= 66.3);
    const metrics = await h.edge.evaluate(() => {
      const q = document.getElementById("queuePanel").getBoundingClientRect();
      const p = document.querySelector(".player").getBoundingClientRect();
      return {
        queueItems: document.querySelectorAll(".qitem").length,
        queueWidth: q.width,
        playerBottom: p.bottom,
        curTime: document.getElementById("curTime").textContent,
      };
    });
    assert.equal(metrics.queueItems, 2);
    assert.ok(metrics.queueWidth <= 300);
    assert.ok(metrics.playerBottom <= 520 + 1);
    assert.match(metrics.curTime, /^1:0[6-9]/);
  } finally {
    await h.close();
  }
});

test("T3-09 (F1 x F7): search -> streams -> playback executes purely via local HTTP IPC with zero external network", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const aUrl = h.registerTrackMedia("t309_a", h.fixtures.audioMain);
    h.appServer.setIpcHandler("search", async ({ query }) => [
      { id: "kalah2", name: query, artist: "Aftershine", duration: 80, video: false },
    ]);
    h.appServer.setIpcHandler("streams", async () => ({
      muxed: [],
      video_only: [],
      audio: [{ url: aUrl, mime: h.fixtures.audioMain.mime, bitrate: 176400 }],
      expires_in_seconds: 21600,
    }));

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Kalah 2";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");
    await h.edge.waitFor(() => !document.getElementById("audio").paused && document.getElementById("audio").currentTime > 0.2);

    const reqs = h.appServer.getMediaRequests("t309_a");
    assert.ok(reqs.length >= 1);
    assert.ok(reqs.every((r) => r.status === 206 || r.status === 200));
    assert.deepEqual(h.edge.pageErrors, []);
    assert.deepEqual(h.edge.consoleErrors, []);
  } finally {
    await h.close();
  }
});

test("T3-10 (F6 x F8): split-stream sync stays <= 350ms drift across 1:04 while PiP is active", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const vUrl = h.registerTrackMedia("t310_v", h.fixtures.video16x9);
    const aUrl = h.registerTrackMedia("t310_a", h.fixtures.audioMain);
    h.appServer.setIpcHandler("search", async () => [
      { id: "kalah2-v", name: "Kalah 2", artist: "Aftershine", duration: 80, video: true },
    ]);
    h.appServer.setIpcHandler("streams", async () => ({
      muxed: [],
      video_only: [{ url: vUrl, mime: h.fixtures.video16x9.mime, quality: "360p", height: 180 }],
      audio: [{ url: aUrl, mime: h.fixtures.audioMain.mime, bitrate: 176400 }],
      expires_in_seconds: 21600,
    }));

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Kalah 2";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");
    await h.edge.waitFor(
      () => {
        const v = document.getElementById("video");
        const a = document.getElementById("audio");
        return !v.paused && !a.paused && v.readyState >= 3 && a.readyState >= 3;
      },
      { timeoutMs: 10000, label: "video & audio ready before PiP" }
    );

    await h.edge.clickSelector("#pipBtn");
    await h.edge.waitFor(() => document.pictureInPictureElement === document.getElementById("video"));

    // Introduce slight drift just before 1:04 and verify 500ms split-sync loop corrects it across 1:04
    await h.edge.evaluate(() => {
      document.getElementById("audio").currentTime = 63.0;
      document.getElementById("video").currentTime = 61.5;
    });
    await h.edge.waitFor(
      () => {
        const a = document.getElementById("audio");
        const v = document.getElementById("video");
        return a.currentTime >= 65.0 && Math.abs(v.currentTime - a.currentTime) <= 0.35;
      },
      { timeoutMs: 8000, label: "split sync corrected across 1:04 during PiP" }
    );

    const res = await h.edge.evaluate(() => ({
      objectFit: getComputedStyle(document.getElementById("video")).objectFit,
      pipActive: document.getElementById("pipBtn").classList.contains("active"),
    }));
    assert.equal(res.objectFit, "contain");
    assert.equal(res.pipActive, true);
  } finally {
    await h.close();
  }
});
