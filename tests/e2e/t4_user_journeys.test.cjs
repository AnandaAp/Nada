"use strict";

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { createBrowserHarness } = require("./lib/harness.cjs");

test("T4-01: Consecutive audio tracks past 1:04 ('Kalah 2' flow: initial -> auto-next -> manual next -> prev)", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const f1 = h.fixtures.audioMain;
    const f2 = h.fixtures.audioAlt;
    const u1 = h.registerTrackMedia("t401_t1", f1);
    const u2 = h.registerTrackMedia("t401_kalah2", f2);
    const u3 = h.registerTrackMedia("t401_t3", f1);

    h.appServer.setIpcHandler("search", async () => [
      { id: "tr1", name: "Kita Usahakan Lagi", artist: "Batas Senja", duration: 80, video: false },
      { id: "qGXY_kO0K7Y", name: "Kalah 2", artist: "Aftershine", duration: 78, video: false },
      { id: "tr3", name: "Mangu", artist: "Fourtwnty", duration: 80, video: false },
    ]);
    h.appServer.setIpcHandler("streams", async ({ videoId }) => {
      const url = videoId === "tr1" ? u1 : videoId === "qGXY_kO0K7Y" ? u2 : u3;
      const mime = videoId === "qGXY_kO0K7Y" ? f2.mime : f1.mime;
      return { muxed: [], video_only: [], audio: [{ url, mime, bitrate: 176400 }], expires_in_seconds: 21600 };
    });

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Kalah 2";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 3);

    // 1. Initial track plays across 1:04 (64s)
    await h.edge.clickSelector('.result[data-id="tr1"]');
    await h.edge.waitFor(() => document.getElementById("audio").readyState >= 3);
    await h.edge.evaluate(() => { document.getElementById("audio").currentTime = 62; });
    await h.edge.waitFor(() => document.getElementById("audio").currentTime >= 65.5, {
      timeoutMs: 8000,
      label: "Track 1 past 1:04",
    });

    // 2. Natural track end triggers auto-next to Track 2 ("Kalah 2")
    await h.edge.evaluate(() => {
      const a = document.getElementById("audio");
      a.currentTime = a.duration - 0.35;
    });
    await h.edge.waitFor(
      () => document.getElementById("pTitle").textContent === "Kalah 2" && document.getElementById("audio").readyState >= 3,
      { timeoutMs: 8000, label: "auto-next to Kalah 2" }
    );

    // Play Kalah 2 continuously past 1:04
    await h.edge.evaluate(() => { document.getElementById("audio").currentTime = 62; });
    await h.edge.waitFor(() => document.getElementById("audio").currentTime >= 66.0 && !document.getElementById("audio").paused, {
      timeoutMs: 8000,
      label: "Kalah 2 past 1:04",
    });

    // 3. Manual next() to Track 3 past 1:04, then prev() back to Kalah 2
    await h.edge.clickSelector("#nextBtn");
    await h.edge.waitFor(
      () => document.getElementById("pTitle").textContent === "Mangu" && document.getElementById("audio").readyState >= 3,
      { timeoutMs: 8000, label: "manual next to Mangu" }
    );
    await h.edge.evaluate(() => { document.getElementById("audio").currentTime = 65; });
    await h.edge.waitFor(() => document.getElementById("audio").currentTime >= 65.5);

    // At t > 3s, first prev() rewinds to 0; at t <= 3s, second prev() returns to Kalah 2
    await h.edge.clickSelector("#prevBtn");
    await h.edge.waitFor(() => document.getElementById("audio").currentTime < 2);
    await h.edge.clickSelector("#prevBtn");
    await h.edge.waitFor(
      () => document.getElementById("pTitle").textContent === "Kalah 2" && document.getElementById("audio").readyState >= 3,
      { timeoutMs: 8000, label: "prev back to Kalah 2" }
    );
  } finally {
    await h.close();
  }
});

test("T4-02: Video mode split sync past 1:04 + forward/back seeks + 16:9/4:3/9:16 uncropped + right-click & button PiP", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const aUrl = h.registerTrackMedia("t402_a", h.fixtures.audioMain);
    const v16 = h.registerTrackMedia("t402_v16", h.fixtures.video16x9);
    const v43 = h.registerTrackMedia("t402_v43", h.fixtures.video4x3);
    const v916 = h.registerTrackMedia("t402_v916", h.fixtures.video9x16);

    h.appServer.setIpcHandler("search", async () => [
      { id: "v16", name: "16:9 Video", artist: "Art", duration: 80, video: true },
      { id: "v43", name: "4:3 Video", artist: "Art", duration: 78, video: true },
      { id: "v916", name: "9:16 Video", artist: "Art", duration: 78, video: true },
    ]);
    h.appServer.setIpcHandler("streams", async ({ videoId }) => {
      const vUrl = videoId === "v16" ? v16 : videoId === "v43" ? v43 : v916;
      const fx = videoId === "v16" ? h.fixtures.video16x9 : videoId === "v43" ? h.fixtures.video4x3 : h.fixtures.video9x16;
      return {
        muxed: [],
        video_only: [{ url: vUrl, mime: fx.mime, quality: "360p", height: fx.height }],
        audio: [{ url: aUrl, mime: h.fixtures.audioMain.mime, bitrate: 176400 }],
        expires_in_seconds: 21600,
      };
    });

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Video";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 3);

    for (const [id, expW, expH] of [["v16", 320, 180], ["v43", 320, 240], ["v916", 180, 320]]) {
      await h.edge.clickSelector(`.result[data-id="${id}"]`);
      await h.edge.waitFor(
        `(() => { const v = document.getElementById("video"); return v.videoWidth === ${expW} && v.videoHeight === ${expH} && v.readyState >= 3; })()`,
        { timeoutMs: 10000, label: `aspect ${id}` }
      );

      const check = await h.edge.evaluate(() => {
        const sr = document.getElementById("videoStage").getBoundingClientRect();
        const vr = document.getElementById("video").getBoundingClientRect();
        const v = document.getElementById("video");
        return {
          objectFit: getComputedStyle(v).objectFit,
          stageW: sr.width,
          stageH: sr.height,
          inside: vr.left >= sr.left - 1 && vr.right <= sr.right + 1 && vr.top >= sr.top - 1 && vr.bottom <= sr.bottom + 1,
          pipOk: !v.disablePictureInPicture,
          centerId: document.elementFromPoint(vr.left + vr.width / 2, vr.top + vr.height / 2)?.id,
        };
      });
      assert.equal(check.objectFit, "contain");
      assert.ok(Math.abs(check.stageW / check.stageH - 16 / 9) < 0.03);
      assert.equal(check.inside, true);
      assert.equal(check.pipOk, true);
      assert.equal(check.centerId, "video");
    }

    // Forward seek to 72s (>1:04) and back seek to 45s on 9:16 or 16:9 split stream + PiP toggle
    await h.edge.clickSelector('.result[data-id="v16"]');
    await h.edge.waitFor(() => document.getElementById("video").videoWidth === 320);
    await h.edge.evaluate(() => {
      document.getElementById("audio").currentTime = 72;
      document.getElementById("video").currentTime = 72;
    });
    await h.edge.waitFor(() => document.getElementById("audio").currentTime >= 72.3);
    await h.edge.evaluate(() => {
      document.getElementById("audio").currentTime = 45;
      document.getElementById("video").currentTime = 45;
    });
    await h.edge.waitFor(() => document.getElementById("audio").currentTime >= 45.2 && document.getElementById("audio").currentTime < 50);
    await h.edge.clickSelector("#pipBtn");
    await h.edge.waitFor(() => document.pictureInPictureElement === document.getElementById("video"));
    await h.edge.clickSelector("#pipBtn");
    await h.edge.waitFor(() => document.pictureInPictureElement === null);
  } finally {
    await h.close();
  }
});

test("T4-03: Adversarial 1:04 cutoff (drop+502 on Track 1 at 64s + 403 candidate fallback on Track 2)", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640, stubYT: true });
  try {
    const f1 = h.fixtures.audioMain;
    const f2 = h.fixtures.audioAlt;
    const cutoff = f1.byteOffsetAtSec(64);
    const t1V1 = h.registerTrackMedia("t403_1_bad", f1, {
      dropAfterBytes: cutoff,
      errorStatus: 502,
      errorAfterBytes: cutoff,
    });
    const t1V2 = h.registerTrackMedia("t403_1_ok", f1);
    const t2Cand1 = h.registerTrackMedia("t403_2_403", f2, { errorStatus: 403 });
    const t2Cand2 = h.registerTrackMedia("t403_2_ok", f2);

    let t1Streams = 0;
    h.appServer.setIpcHandler("search", async () => [
      { id: "tr1", name: "Cutoff Track 1", artist: "Art", duration: 80, video: false },
      { id: "tr2", name: "Fallback Track 2", artist: "Art", duration: 78, video: false },
    ]);
    h.appServer.setIpcHandler("streams", async ({ videoId }) => {
      if (videoId === "tr1") {
        t1Streams++;
        return {
          muxed: [],
          video_only: [],
          audio: [{ url: t1Streams === 1 ? t1V1 : t1V2, mime: f1.mime, bitrate: 176400 }],
          expires_in_seconds: 21600,
        };
      }
      return {
        muxed: [],
        video_only: [],
        audio: [
          { url: t2Cand1, mime: f2.mime, bitrate: 176400 },
          { url: t2Cand2, mime: f2.mime, bitrate: 160000 },
        ],
        expires_in_seconds: 21600,
      };
    });

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Cutoff";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 2);
    await h.edge.clickSelector('.result[data-id="tr1"]');
    await h.edge.waitFor(() => document.getElementById("audio").readyState >= 2);
    await h.edge.evaluate(() => { document.getElementById("audio").currentTime = 62; });

    await h.edge.waitFor(
      () => document.getElementById("audio").src.includes("t403_1_ok") && document.getElementById("audio").currentTime >= 65.5,
      { timeoutMs: 15000, label: "Track 1 recovered past 1:04" }
    );

    // Advance to Track 2 -> cand1 returns 403 -> cand2 plays past 1:04
    await h.edge.clickSelector("#nextBtn");
    await h.edge.waitFor(
      () => document.getElementById("audio").src.includes("t403_2_ok") && document.getElementById("audio").readyState >= 3,
      { timeoutMs: 12000, label: "Track 2 recovered onto cand2" }
    );
    await h.edge.evaluate(() => { document.getElementById("audio").currentTime = 63; });
    await h.edge.waitFor(() => document.getElementById("audio").currentTime >= 65.5 && !document.getElementById("audio").paused);
    const ytLoads = await h.edge.evaluate(() => window.__NADA_YT_LOADS__.length);
    assert.equal(ytLoads, 0, "ytPlayer fallback must never fire during Track 1/2 recovery");
  } finally {
    await h.close();
  }
});

test("T4-04: Rapid navigation, 12-track skip with radio continuation, and Track 1 replay under R3 hygiene", async () => {
  const h = await createBrowserHarness({
    width: 430,
    height: 640,
    ipcHandlers: {
      home_section: async ({ section }) => ({
        items: section === "releases"
          ? [{ id: "alb1", name: "Album One", artist: "Aftershine", kind: "album", video: false }]
          : [],
      }),
      collection_tracks: async () => [
        { id: "c1", name: "Coll Track 1", artist: "Aftershine", duration: 80, video: false },
      ],
    },
  });
  try {
    const f = h.fixtures.audioMain;
    const urls = Array.from({ length: 12 }, (_, i) => h.registerTrackMedia(`t404_${i + 1}`, f));

    h.appServer.setIpcHandler("search", async () => [
      { id: "k1", name: "Track 01", artist: "Aftershine", duration: 80, video: false },
      { id: "k2", name: "Track 02", artist: "Aftershine", duration: 80, video: false },
    ]);
    h.appServer.setIpcHandler("radio", async () =>
      Array.from({ length: 10 }, (_, i) => ({
        id: `k${i + 3}`,
        name: `Track ${String(i + 3).padStart(2, "0")}`,
        artist: "Aftershine",
        duration: 80,
        video: false,
      }))
    );
    h.appServer.setIpcHandler("streams", async ({ videoId }) => {
      const num = Number(videoId.replace(/\D/g, "")) || 1;
      const url = urls[(num - 1) % urls.length];
      return { muxed: [], video_only: [], audio: [{ url, mime: f.mime, bitrate: 176400 }], expires_in_seconds: 21600 };
    });

    // Open collection -> go home -> search -> clear -> search -> play Track 01
    await h.edge.waitFor(() => document.querySelectorAll(".homecard").length === 1);
    await h.edge.clickSelector(".homecard");
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector("#backHome");
    await h.edge.evaluate(() => {
      const inp = document.getElementById("search");
      inp.value = "Aftershine";
      inp.dispatchEvent(new Event("input", { bubbles: true }));
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 2);
    await h.edge.clickSelector("#clearSearch");
    await h.edge.evaluate(() => {
      const inp = document.getElementById("search");
      inp.value = "Aftershine";
      inp.dispatchEvent(new Event("input", { bubbles: true }));
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 2);
    await h.edge.clickSelector('.result[data-id="k1"]');
    await h.edge.waitFor(() => document.getElementById("pTitle").textContent === "Track 01");

    // Skip across 11 next() calls (triggers radio continuation to reach 12 tracks)
    for (let step = 2; step <= 12; step++) {
      await h.edge.clickSelector("#nextBtn");
      const expected = `Track ${String(step).padStart(2, "0")}`;
      await h.edge.waitFor(`document.getElementById("pTitle").textContent === ${JSON.stringify(expected)}`, {
        timeoutMs: 8000,
        label: `skip to ${expected}`,
      });
    }

    // Open queue panel and replay Track 01 from queue
    await h.edge.clickSelector("#queueBtn");
    await h.edge.waitFor(() => document.querySelectorAll(".qitem").length === 12);
    await h.edge.clickSelector(".qitem:first-child");
    await h.edge.waitFor(
      () =>
        document.getElementById("pTitle").textContent === "Track 01" &&
        document.getElementById("audio").src.includes("t404_1") &&
        !document.getElementById("audio").paused,
      { timeoutMs: 8000, label: "replay Track 01" }
    );
    assert.deepEqual(h.edge.pageErrors, []);
    assert.deepEqual(h.edge.consoleErrors, []);
  } finally {
    await h.close();
  }
});
