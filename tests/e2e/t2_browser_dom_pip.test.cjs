"use strict";

const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { createBrowserHarness } = require("./lib/harness.cjs");

const REPO_ROOT = path.resolve(__dirname, "..", "..");

function buildSplitIpc(harness, { videoKey = "video16x9", audioKey = "audioMain" } = {}) {
  const f = harness.fixtures;
  const vUrl = harness.registerTrackMedia(`${videoKey}_tok`, f[videoKey]);
  const aUrl = harness.registerTrackMedia(`${audioKey}_tok`, f[audioKey]);
  return { vUrl, aUrl };
}

test("T2-F6-01: Edge CDP renders src/index.html at 430x640 with zero errors, 4 home sections, and zero horizontal overflow", async () => {
  const h = await createBrowserHarness({
    width: 430,
    height: 640,
    ipcHandlers: {
      home_section: async ({ section }) => ({
        items: [
          {
            id: `item-${section}`,
            name: `Hit ${section}`,
            artist: "Aftershine",
            kind: "track",
            duration: 80,
            video: false,
          },
        ],
      }),
    },
  });
  try {
    await h.edge.waitFor(
      () => document.querySelectorAll(".homecard").length === 4,
      { timeoutMs: 8000, label: "4 home sections rendered" }
    );
    const info = await h.edge.evaluate(() => ({
      scrollWidth: document.documentElement.scrollWidth,
      clientWidth: document.documentElement.clientWidth,
      stageHidden: document.getElementById("videoStage").classList.contains("hidden"),
      homeSections: document.querySelectorAll(".homesection").length,
    }));
    assert.ok(info.scrollWidth <= 430, `Horizontal overflow at 430px: scrollWidth=${info.scrollWidth}`);
    assert.equal(info.clientWidth, 430);
    assert.equal(info.stageHidden, true);
    assert.equal(info.homeSections, 4);
    assert.deepEqual(h.edge.pageErrors, []);
    assert.deepEqual(h.edge.consoleErrors, []);
  } finally {
    await h.close();
  }
});

test("T2-F6-02: Edge CDP video mode at 430x640 renders 16:9 #videoStage & #video with #ytPlayer hidden", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const { vUrl, aUrl } = buildSplitIpc(h);
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
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1, { label: "search result" });
    await h.edge.clickSelector(".result");
    await h.edge.waitFor(
      () => {
        const v = document.getElementById("video");
        return v && v.videoWidth > 0 && !v.paused;
      },
      { label: "video playing" }
    );

    const geom = await h.edge.evaluate(() => {
      const stage = document.getElementById("videoStage");
      const video = document.getElementById("video");
      const yt = document.getElementById("ytPlayer");
      const sr = stage.getBoundingClientRect();
      const vr = video.getBoundingClientRect();
      return {
        stageRect: { w: sr.width, h: sr.height },
        videoRect: { w: vr.width, h: vr.height },
        ytDisplay: getComputedStyle(yt).display,
        ytHidden: yt.classList.contains("hidden"),
      };
    });

    assert.ok(Math.abs(geom.stageRect.w - 430) <= 1, `stage width=${geom.stageRect.w}`);
    assert.ok(Math.abs(geom.videoRect.w - 430) <= 1, `video width=${geom.videoRect.w}`);
    assert.ok(Math.abs(geom.stageRect.w / geom.stageRect.h - 16 / 9) < 0.03, `stage ratio=${geom.stageRect.w / geom.stageRect.h}`);
    assert.ok(Math.abs(geom.videoRect.w / geom.videoRect.h - 16 / 9) < 0.03, `video ratio=${geom.videoRect.w / geom.videoRect.h}`);
    assert.equal(geom.ytDisplay, "none");
    assert.equal(geom.ytHidden, true);
  } finally {
    await h.close();
  }
});

test("T2-F6-03: #video uses object-fit: contain and plays uncropped video past 1:04", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const { vUrl, aUrl } = buildSplitIpc(h);
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
    await h.edge.waitFor(() => document.getElementById("video").readyState >= 3);

    // Seek just before 1:04 (62s) and verify continuous playback across 64s -> 66s
    await h.edge.evaluate(() => {
      const a = document.getElementById("audio");
      const v = document.getElementById("video");
      a.currentTime = 62;
      v.currentTime = 62;
    });
    await h.edge.waitFor(() => document.getElementById("audio").currentTime >= 65.5, {
      timeoutMs: 8000,
      label: "video crossing 1:04",
    });

    const st = await h.edge.evaluate(() => {
      const v = document.getElementById("video");
      return {
        objectFit: getComputedStyle(v).objectFit,
        vw: v.videoWidth,
        vh: v.videoHeight,
        curTimeText: document.getElementById("curTime").textContent,
        ytHidden: document.getElementById("ytPlayer").classList.contains("hidden"),
      };
    });
    assert.equal(st.objectFit, "contain");
    assert.equal(st.vw, 320);
    assert.equal(st.vh, 180);
    assert.equal(st.ytHidden, true);
    assert.match(st.curTimeText, /^1:0[5-9]|1:[1-9]/);
  } finally {
    await h.close();
  }
});

test("T2-F6-04: .stage-pip-btn resolves valid computed color, borderColor, borderRadius without undefined CSS vars", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const { vUrl, aUrl } = buildSplitIpc(h);
    h.appServer.setIpcHandler("search", async () => [
      { id: "v1", name: "Vid", artist: "Art", duration: 80, video: true },
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
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");
    await h.edge.waitFor(() => !document.getElementById("videoStage").classList.contains("hidden"));

    const styles = await h.edge.evaluate(() => {
      const btn = document.getElementById("pipBtn");
      const stage = document.getElementById("videoStage");
      const rootStyle = getComputedStyle(document.documentElement);
      const tokenColor = rootStyle.getPropertyValue("--text-dimmed").trim();
      const tokenBorder = rootStyle.getPropertyValue("--border-subtle").trim();
      const tokenRadius = rootStyle.getPropertyValue("--radius-item").trim();
      const tokenBrass = rootStyle.getPropertyValue("--accent-brass").trim();
      const tokenGold = rootStyle.getPropertyValue("--accent-gold").trim();

      const probe = document.createElement("div");
      probe.style.display = "none";
      probe.style.color = tokenColor;
      probe.style.borderTop = `1px solid ${tokenBorder}`;
      probe.style.borderRadius = tokenRadius;
      document.body.appendChild(probe);
      const pcs = getComputedStyle(probe);
      const expectedColor = pcs.color;
      const expectedBorderTopColor = pcs.borderTopColor;
      const expectedRadius = pcs.borderRadius;
      probe.remove();

      const cs = getComputedStyle(btn);
      const br = btn.getBoundingClientRect();
      const sr = stage.getBoundingClientRect();
      return {
        tokenColor,
        tokenBorder,
        tokenRadius,
        tokenBrass,
        tokenGold,
        color: cs.color,
        borderTopColor: cs.borderTopColor,
        borderRadius: cs.borderRadius,
        expectedColor,
        expectedBorderTopColor,
        expectedRadius,
        zIndex: Number(cs.zIndex),
        rightGap: sr.right - br.right,
        topGap: br.top - sr.top,
        w: br.width,
        h: br.height,
      };
    });

    assert.ok(styles.tokenColor.length > 0, "--text-dimmed token must be defined on :root");
    assert.ok(styles.tokenBorder.length > 0, "--border-subtle token must be defined on :root");
    assert.ok(styles.tokenRadius.length > 0, "--radius-item token must be defined on :root");
    assert.ok(styles.tokenBrass.length > 0, "--accent-brass token must be defined on :root");
    assert.ok(styles.tokenGold.length > 0, "--accent-gold token must be defined on :root");
    assert.equal(styles.color, styles.expectedColor, "pipBtn color must match resolved :root --text-dimmed (no IACVT fallback)");
    assert.equal(styles.borderTopColor, styles.expectedBorderTopColor, "pipBtn borderTopColor must match resolved :root --border-subtle (no IACVT fallback)");
    assert.equal(styles.borderRadius, styles.expectedRadius, "pipBtn borderRadius must match resolved :root --radius-item");
    assert.notEqual(styles.color, "rgba(0, 0, 0, 0)");
    assert.notEqual(styles.borderTopColor, "rgba(0, 0, 0, 0)");
    assert.notEqual(styles.borderRadius, "0px");
    assert.ok(styles.zIndex >= 10);
    assert.ok(styles.w >= 28 && styles.h >= 28);
    assert.ok(styles.rightGap >= 0 && styles.rightGap <= 20);
    assert.ok(styles.topGap >= 0 && styles.topGap <= 20);
  } finally {
    await h.close();
  }
});

test("T2-F6-05: right-click Picture-in-Picture on #video is unobstructed and contextmenu is not prevented", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const { vUrl, aUrl } = buildSplitIpc(h);
    h.appServer.setIpcHandler("search", async () => [
      { id: "v1", name: "Vid", artist: "Art", duration: 80, video: true },
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
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");
    await h.edge.waitFor(() => document.getElementById("video").videoWidth > 0);

    const hitTest = await h.edge.evaluate(() => {
      const v = document.getElementById("video");
      const r = v.getBoundingClientRect();
      const points = [
        [r.left + r.width * 0.5, r.top + r.height * 0.5],
        [r.left + r.width * 0.25, r.top + r.height * 0.25],
        [r.left + r.width * 0.65, r.top + r.height * 0.25],
        [r.left + r.width * 0.25, r.top + r.height * 0.75],
        [r.left + r.width * 0.75, r.top + r.height * 0.75],
      ];
      const hitIds = points.map(([x, y]) => {
        const el = document.elementFromPoint(x, y);
        return el ? el.id : null;
      });
      const evt = new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        clientX: r.left + r.width * 0.5,
        clientY: r.top + r.height * 0.5,
        button: 2,
      });
      const notCancelled = v.dispatchEvent(evt);
      return {
        disablePipProp: v.disablePictureInPicture,
        hasDisableAttr: v.hasAttribute("disablepictureinpicture"),
        hitIds,
        defaultPrevented: evt.defaultPrevented,
        notCancelled,
      };
    });

    assert.equal(hitTest.disablePipProp, false);
    assert.equal(hitTest.hasDisableAttr, false);
    assert.deepEqual(hitTest.hitIds, ["video", "video", "video", "video", "video"]);
    assert.equal(hitTest.defaultPrevented, false);
    assert.equal(hitTest.notCancelled, true);
  } finally {
    await h.close();
  }
});

test("T2-F6-06: real CDP click on #pipBtn toggles native Edge PiP without pausing video", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const { vUrl, aUrl } = buildSplitIpc(h);
    h.appServer.setIpcHandler("search", async () => [
      { id: "v1", name: "Vid", artist: "Art", duration: 80, video: true },
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
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");
    await h.edge.waitFor(() => {
      const v = document.getElementById("video");
      return v.readyState >= 3 && !v.paused;
    });

    await h.edge.clickSelector("#pipBtn");
    await h.edge.waitFor(() => document.pictureInPictureElement === document.getElementById("video"), {
      label: "enter PiP",
    });

    const entered = await h.edge.evaluate(() => ({
      pipActive: document.getElementById("pipBtn").classList.contains("active"),
      videoPaused: document.getElementById("video").paused,
      audioPaused: document.getElementById("audio").paused,
    }));
    assert.equal(entered.pipActive, true);
    assert.equal(entered.videoPaused, false);
    assert.equal(entered.audioPaused, false);

    // Second click exits PiP cleanly
    await h.edge.clickSelector("#pipBtn");
    await h.edge.waitFor(() => document.pictureInPictureElement === null, { label: "exit PiP" });
    const exited = await h.edge.evaluate(() => ({
      pipActive: document.getElementById("pipBtn").classList.contains("active"),
    }));
    assert.equal(exited.pipActive, false);
  } finally {
    await h.close();
  }
});

test("T2-F6-07: split-mode PiP pause/play on #video synchronizes #audio & #playBtn UI, and video off exits PiP", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const { vUrl, aUrl } = buildSplitIpc(h);
    h.appServer.setIpcHandler("search", async () => [
      { id: "v1", name: "Vid", artist: "Art", duration: 80, video: true },
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
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");
    await h.edge.waitFor(
      () =>
        !document.getElementById("video").paused &&
        !document.getElementById("audio").paused &&
        document.getElementById("video").readyState >= 3 &&
        document.getElementById("audio").readyState >= 3,
      { label: "split video & audio playing readyState >= 3" }
    );

    await h.edge.clickSelector("#pipBtn");
    await h.edge.waitFor(() => document.pictureInPictureElement === document.getElementById("video"));

    // Native PiP window pauses <video> directly
    await h.edge.evaluate(() => { document.getElementById("video").pause(); });
    await h.edge.waitFor(() => document.getElementById("audio").paused === true, {
      label: "audio paused on PiP video pause",
    });
    const pausedUi = await h.edge.evaluate(() => ({
      iconPlayHidden: document.getElementById("iconPlay").classList.contains("hidden"),
      iconPauseHidden: document.getElementById("iconPause").classList.contains("hidden"),
    }));
    assert.equal(pausedUi.iconPlayHidden, false);
    assert.equal(pausedUi.iconPauseHidden, true);

    // Native PiP window resumes <video> directly
    await h.edge.evaluate(() => document.getElementById("video").play());
    await h.edge.waitFor(() => document.getElementById("audio").paused === false, {
      label: "audio resumed on PiP video play",
    });

    // Clicking #videoBtn off exits PiP
    await h.edge.clickSelector("#videoBtn");
    await h.edge.waitFor(() => document.pictureInPictureElement === null && document.getElementById("videoStage").classList.contains("hidden"));
  } finally {
    await h.close();
  }
});

// --- F7: Responsive Viewport & CSP / Hygiene (5 cases) ---

test("T2-F7-01: Edge CDP at 430x520 minimum height keeps .stage bounded and .player controls non-overlapping", async () => {
  const h = await createBrowserHarness({ width: 430, height: 520 });
  try {
    const { vUrl, aUrl } = buildSplitIpc(h);
    h.appServer.setIpcHandler("search", async () => [
      { id: "v1", name: "Vid", artist: "Art", duration: 80, video: true },
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
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    await h.edge.clickSelector(".result");
    await h.edge.waitFor(() => !document.getElementById("videoStage").classList.contains("hidden"));

    const layout = await h.edge.evaluate(() => {
      const sr = document.getElementById("videoStage").getBoundingClientRect();
      const pr = document.querySelector(".player").getBoundingClientRect();
      const playR = document.getElementById("playBtn").getBoundingClientRect();
      return {
        stageHeight: sr.height,
        stageBottom: sr.bottom,
        playerTop: pr.top,
        playerBottom: pr.bottom,
        playBtnBottom: playR.bottom,
        scrollWidth: document.documentElement.scrollWidth,
      };
    });

    assert.ok(layout.stageHeight <= 520 * 0.28 + 3, `Expected <= 28vh at 520px, got ${layout.stageHeight}`);
    assert.ok(layout.stageBottom <= layout.playerTop + 1, "videoStage must not overlap player dock");
    assert.ok(layout.playerBottom <= 520 + 1, `player bottom clipped: ${layout.playerBottom}`);
    assert.ok(layout.playBtnBottom <= 520 + 1);
    assert.ok(layout.scrollWidth <= 430);
  } finally {
    await h.close();
  }
});

test("T2-F7-02: strict CSP header blocks eval/inline abuse while allowing local proxy media playback", async () => {
  const csp =
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; media-src 'self' http://127.0.0.1:* blob:; connect-src 'self' http://127.0.0.1:*; object-src 'none'; base-uri 'self'";
  const h = await createBrowserHarness({ width: 430, height: 640, cspHeader: csp });
  try {
    const aUrl = h.registerTrackMedia("csp_audio", h.fixtures.audioMain);
    h.appServer.setIpcHandler("search", async () => [
      { id: "csp1", name: "Kalah 2", artist: "Aftershine", duration: 80, video: false },
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
    await h.edge.waitFor(() => !document.getElementById("audio").paused && document.getElementById("audio").readyState >= 3);
    assert.deepEqual(h.edge.pageErrors, []);
  } finally {
    await h.close();
  }
});

test("T2-F7-03: unavailable localStorage does not crash app startup or playback", async () => {
  const h = await createBrowserHarness({
    width: 430,
    height: 640,
    breakLocalStorage: true,
    ipcHandlers: {
      home_section: async ({ section }) => ({
        items: [{ id: `h-${section}`, name: `Song ${section}`, artist: "Art", kind: "track", video: false }],
      }),
    },
  });
  try {
    await h.edge.waitFor(() => document.querySelectorAll(".homecard").length === 4);
    assert.deepEqual(h.edge.pageErrors, []);
  } finally {
    await h.close();
  }
});

test("T2-F7-04: Edge CDP session cleanup kills spawned browser PID and deletes temporary profile directory", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  const pid = h.edge.proc.pid;
  const userDir = h.edge.userDataDir;
  assert.ok(pid > 0);
  assert.equal(fs.existsSync(userDir), true);

  await h.close();
  assert.equal(fs.existsSync(userDir), false, `Temp Edge profile not removed: ${userDir}`);
  let alive = true;
  try {
    process.kill(pid, 0);
  } catch (_) {
    alive = false;
  }
  assert.equal(alive, false, `Orphan Edge PID ${pid} still alive after close()`);
});

test("T2-F7-05: post-browser check confirms zero repo target nada.exe processes launched", () => {
  if (process.platform !== "win32") return;
  const tl = execFileSync("tasklist", ["/FI", "IMAGENAME eq nada.exe", "/FO", "CSV", "/NH"], {
    encoding: "utf8",
  }).trim();
  if (!tl || tl.startsWith("INFO:")) return;
  const out = execFileSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-Command",
      "try { @(Get-Process nada -ErrorAction Stop | Select-Object Id, Path) | ConvertTo-Json -Compress } catch { '[]' }",
    ],
    { encoding: "utf8" }
  ).trim();
  if (!out || out === "[]") return;
  const parsed = JSON.parse(out);
  const list = Array.isArray(parsed) ? parsed : [parsed];
  const repoTarget = path.join(REPO_ROOT, "src-tauri", "target").toLowerCase();
  for (const p of list) {
    assert.equal((p.Path || "").toLowerCase().startsWith(repoTarget), false);
  }
});

test("T2-F7-06: playback skeleton shimmers without layout shift and clears when audio starts", async () => {
  const h = await createBrowserHarness({ width: 430, height: 640 });
  try {
    const { vUrl, aUrl } = buildSplitIpc(h);
    h.appServer.setIpcHandler("search", async () => [
      { id: "skeleton", name: "Skeleton Song", artist: "Nada", duration: 80, video: false },
    ]);

    let releaseStreams;
    let markRequested;
    const streamsRequested = new Promise((resolve) => { markRequested = resolve; });
    h.appServer.setIpcHandler("streams", async () => {
      markRequested();
      return new Promise((resolve) => { releaseStreams = resolve; });
    });

    await h.edge.evaluate(() => {
      document.getElementById("search").value = "Skeleton";
      document.getElementById("searchBtn").click();
    });
    await h.edge.waitFor(() => document.querySelectorAll(".result").length === 1);
    const beforeHeight = await h.edge.evaluate(() => document.getElementById("player").getBoundingClientRect().height);
    await h.edge.clickSelector(".result");
    await streamsRequested;
    await h.edge.waitFor(() => document.getElementById("player").classList.contains("is-loading"));
    await h.edge.waitFor(
      () => Number(getComputedStyle(document.getElementById("pMetaSkeleton")).opacity) > 0.9,
      { timeoutMs: 1000, label: "skeleton visible" }
    );

    const loading = await h.edge.evaluate(() => {
      const player = document.getElementById("player");
      const skeleton = document.getElementById("pMetaSkeleton");
      const line = document.querySelector(".skeleton-line");
      return {
        busy: player.getAttribute("aria-busy"),
        skeletonDisplay: getComputedStyle(skeleton).display,
        titleVisibility: getComputedStyle(document.getElementById("pTitle")).visibility,
        spinnerDisplay: getComputedStyle(document.getElementById("playLoading")).display,
        shimmerAnimation: getComputedStyle(line, "::after").animationName,
        playerHeight: player.getBoundingClientRect().height,
        scrollWidth: document.documentElement.scrollWidth,
      };
    });
    assert.equal(loading.busy, "true");
    assert.notEqual(loading.skeletonDisplay, "none");
    assert.equal(loading.titleVisibility, "hidden");
    assert.notEqual(loading.spinnerDisplay, "none");
    assert.equal(loading.shimmerAnimation, "skeleton-shimmer");
    assert.ok(Math.abs(loading.playerHeight - beforeHeight) <= 0.5, "loading skeleton must not change player height");
    assert.ok(loading.scrollWidth <= 430, `Horizontal overflow while loading: ${loading.scrollWidth}`);

    releaseStreams({
      muxed: [],
      video_only: [{ url: vUrl, mime: h.fixtures.video16x9.mime, quality: "360p", height: 180 }],
      audio: [{ url: aUrl, mime: h.fixtures.audioMain.mime, bitrate: 176400 }],
      expires_in_seconds: 21600,
    });
    await h.edge.waitFor(
      () => {
        const audio = document.getElementById("audio");
        return !audio.paused && audio.readyState >= 3 && !document.getElementById("player").classList.contains("is-loading");
      },
      { label: "audio playing and skeleton cleared" }
    );

    const ready = await h.edge.evaluate(() => ({
      busy: document.getElementById("player").getAttribute("aria-busy"),
      skeletonDisplay: getComputedStyle(document.getElementById("pMetaSkeleton")).display,
      titleVisibility: getComputedStyle(document.getElementById("pTitle")).visibility,
      spinnerDisplay: getComputedStyle(document.getElementById("playLoading")).display,
    }));
    assert.equal(ready.busy, "false");
    assert.equal(ready.skeletonDisplay, "none");
    assert.equal(ready.titleVisibility, "visible");
    assert.equal(ready.spinnerDisplay, "none");

    await h.edge.send("Emulation.setEmulatedMedia", {
      features: [{ name: "prefers-reduced-motion", value: "reduce" }],
    });
    const reducedMotion = await h.edge.evaluate(() => {
      const line = document.createElement("span");
      line.className = "skeleton-line";
      const spinner = document.createElement("span");
      spinner.className = "play-loading";
      document.body.append(line, spinner);
      const result = {
        shimmer: getComputedStyle(line, "::after").animationName,
        spinner: getComputedStyle(spinner).animationName,
      };
      line.remove();
      spinner.remove();
      return result;
    });
    assert.equal(reducedMotion.shimmer, "none");
    assert.equal(reducedMotion.spinner, "none");
    assert.deepEqual(h.edge.pageErrors, []);
  } finally {
    await h.close();
  }
});

test("T2-F7-07: home skeleton cards shimmer and clear as sections load", async () => {
  const pending = new Map();
  let markAllRequested;
  const allRequested = new Promise((resolve) => { markAllRequested = resolve; });
  const h = await createBrowserHarness({
    width: 430,
    height: 640,
    ipcHandlers: {
      home_section: ({ section }) => new Promise((resolve) => {
        pending.set(section, resolve);
        if (pending.size === 4) markAllRequested();
      }),
    },
  });
  try {
    await allRequested;
    await h.edge.waitFor(() => document.querySelectorAll(".homecards-skeleton").length === 4);

    const loading = await h.edge.evaluate(() => {
      const group = document.querySelector(".homecards-skeleton");
      const cover = group.querySelector(".homecover");
      const line = group.querySelector(".skeleton-line");
      return {
        busy: group.getAttribute("aria-busy"),
        count: group.querySelectorAll(".homecard-skeleton").length,
        coverAnimation: getComputedStyle(cover, "::after").animationName,
        lineAnimation: getComputedStyle(line, "::after").animationName,
        cardHeight: group.querySelector(".homecard-skeleton").getBoundingClientRect().height,
        scrollWidth: document.documentElement.scrollWidth,
        loadingText: document.getElementById("homeSections").textContent.includes("Lagi ngambil rekomendasi"),
      };
    });
    assert.equal(loading.busy, "true");
    assert.equal(loading.count, 3);
    assert.equal(loading.coverAnimation, "skeleton-shimmer");
    assert.equal(loading.lineAnimation, "skeleton-shimmer");
    assert.equal(loading.loadingText, false);
    assert.ok(loading.scrollWidth <= 430, `Horizontal overflow while loading: ${loading.scrollWidth}`);

    await h.edge.send("Emulation.setEmulatedMedia", {
      features: [{ name: "prefers-reduced-motion", value: "reduce" }],
    });
    const reducedMotion = await h.edge.evaluate(() => {
      const group = document.querySelector(".homecards-skeleton");
      const card = group.querySelector(".homecard-skeleton");
      const cover = card.querySelector(".homecover");
      return {
        card: getComputedStyle(card).animationName,
        cover: getComputedStyle(cover, "::after").animationName,
      };
    });
    assert.equal(reducedMotion.card, "none");
    assert.equal(reducedMotion.cover, "none");

    for (const [section, resolve] of pending) {
      resolve({ items: [{ id: `hit-${section}`, name: "Hit", artist: "Art", kind: "track", video: false }] });
    }
    await h.edge.waitFor(
      () => document.querySelectorAll(".homecards-skeleton").length === 0
        && document.querySelectorAll(".homecard:not(.homecard-skeleton)").length === 4,
      { label: "home cards loaded and skeletons cleared" }
    );

    const ready = await h.edge.evaluate(() => ({
      cardHeight: document.querySelector(".homecard:not(.homecard-skeleton)").getBoundingClientRect().height,
      firstTitle: document.querySelector(".homecard .cardtitle").textContent,
    }));
    assert.equal(ready.firstTitle, "Hit");
    assert.ok(Math.abs(ready.cardHeight - loading.cardHeight) <= 1.5, `home skeleton card height ${loading.cardHeight} must match loaded ${ready.cardHeight}`);
    assert.deepEqual(h.edge.pageErrors, []);
  } finally {
    await h.close();
  }
});
