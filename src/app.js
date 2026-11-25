const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const audio = $("audio");
const video = $("video");

function getStorage(key, fallback = null) {
  try {
    return (typeof localStorage !== "undefined" && localStorage.getItem(key)) ?? fallback;
  } catch (_) {
    return fallback;
  }
}

function setStorage(key, val) {
  try {
    if (typeof localStorage !== "undefined") localStorage.setItem(key, val);
  } catch (_) {}
}

const STALL_TIMEOUT_MS = 7000;
const MAX_STREAM_REFRESHES = 2;
const SUSTAINED_PLAYBACK_MS = 10000;

const state = {
  kind: "tracks",
  results: [],
  queue: [],
  index: -1,
  mode: null, // 'audio' | 'muxed' | 'split' | 'yt'
  wantVideo: false,
  streams: null,
  loading: false,
  playbackId: 0,
  starting: false,
  wantPlaying: false,
  seeking: false,
  buffering: false,
  splitBufferingPaused: false,
  splitBufferingSeq: 0,
  recovering: false,
  recoveryTimer: null,
  audioCandidateIdx: 0,
  videoCandidateIdx: 0,
  muxedCandidateIdx: 0,
  streamRefreshCount: 0,
  pendingResumePos: 0,
  lastProgressPos: 0,
  lastRecoveryAt: 0,
  seekSeq: 0,
  radioSeq: 0,
  radioPendingId: 0,
  view: "home",
  viewId: 0,
  homeData: {},
  homeLoading: false,
  homeLoaded: false,
  shuffle: getStorage("nada.shuffle") === "1",
  repeat: getStorage("nada.repeat", "off"), // 'off' | 'all' | 'one'
  originalQueue: null,
};

function shuffleArray(arr) {
  const res = [...arr];
  for (let i = res.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [res[i], res[j]] = [res[j], res[i]];
  }
  return res;
}

function setQueue(items, startIndex = 0) {
  state.radioPendingId = 0;
  if (state.shuffle) {
    state.originalQueue = [...items];
    const picked = items[startIndex];
    const rest = items.filter((_, i) => i !== startIndex);
    const shuffledRest = shuffleArray(rest);
    state.queue = picked ? [picked, ...shuffledRest] : shuffledRest;
    state.index = 0;
  } else {
    state.originalQueue = null;
    state.queue = [...items];
    state.index = startIndex;
  }
}

function updateModeButtonsUI() {
  const repBtn = $("repeatBtn");
  const repBadge = $("repeatBadge");
  const shufBtn = $("shuffleBtn");

  if (repBtn) {
    repBtn.classList.toggle("active", state.repeat !== "off");
    if (state.repeat === "one") {
      repBtn.title = "Ulangi: Satu lagu (r)";
      if (repBadge) repBadge.classList.remove("hidden");
    } else if (state.repeat === "all") {
      repBtn.title = "Ulangi: Semua lagu (r)";
      if (repBadge) repBadge.classList.add("hidden");
    } else {
      repBtn.title = "Ulangi: Mati (r)";
      if (repBadge) repBadge.classList.add("hidden");
    }
  }

  if (shufBtn) {
    shufBtn.classList.toggle("active", !!state.shuffle);
    shufBtn.title = state.shuffle ? "Acak antrean: Aktif (s)" : "Acak antrean: Mati (s)";
  }
}

function toggleRepeat() {
  if (state.repeat === "off") {
    state.repeat = "all";
    toast("Ulangi: Semua lagu");
  } else if (state.repeat === "all") {
    state.repeat = "one";
    toast("Ulangi: Satu lagu");
  } else {
    state.repeat = "off";
    toast("Ulangi: Mati");
  }
  setStorage("nada.repeat", state.repeat);
  updateModeButtonsUI();
}

function toggleShuffle() {
  state.shuffle = !state.shuffle;
  setStorage("nada.shuffle", state.shuffle ? "1" : "0");

  if (state.shuffle) {
    if (state.queue.length > 1) {
      state.originalQueue = [...state.queue];
      const curIndex = state.index;
      if (curIndex >= 0 && curIndex < state.queue.length) {
        const cur = state.queue[curIndex];
        const prevItems = state.queue.slice(0, curIndex);
        const upcoming = state.queue.slice(curIndex + 1);
        const shuffledUpcoming = shuffleArray(upcoming);
        state.queue = [...prevItems, cur, ...shuffledUpcoming];
      } else {
        state.queue = shuffleArray(state.queue);
      }
    }
    toast("Acak antrean: Aktif");
  } else {
    if (state.originalQueue && state.originalQueue.length) {
      const cur = state.queue[state.index];
      state.queue = [...state.originalQueue];
      state.originalQueue = null;
      if (cur) {
        const foundIndex = state.queue.findIndex((t) => t.id === cur.id);
        if (foundIndex !== -1) {
          state.index = foundIndex;
        }
      }
    }
    toast("Acak antrean: Mati");
  }

  updateModeButtonsUI();
  renderQueue();
  highlightPlaying();
}

const homeSections = [
  { id: "community", title: "Trending community playlists", subtitle: "Pilihan komunitas dari YouTube Music" },
  { id: "releases", title: "New releases", subtitle: "Album dan single yang baru dirilis" },
  { id: "indonesia", title: "Today's Indonesian Hits", subtitle: "Daily chart YouTube Music · Indonesia" },
  { id: "international", title: "Today's International hits", subtitle: "Daily chart YouTube Music · Global, termasuk Indonesia" },
];

function renderHome() {
  const root = $("homeSections");
  root.textContent = "";
  $("refreshHome").disabled = state.homeLoading;
  homeSections.forEach((section) => {
    const block = document.createElement("section");
    block.className = "homesection";
    const heading = document.createElement("h2");
    heading.textContent = section.title;
    const sub = document.createElement("p");
    sub.className = "sectionsub";
    sub.textContent = section.subtitle;
    block.append(heading, sub);
    const data = state.homeData[section.id];
    if (!data) {
      const cards = document.createElement("div");
      cards.className = "homecards homecards-skeleton";
      cards.setAttribute("role", "status");
      cards.setAttribute("aria-label", "Lagi ngambil rekomendasi…");
      cards.setAttribute("aria-busy", "true");
      for (let i = 0; i < 3; i++) {
        const card = document.createElement("div");
        card.className = "homecard homecard-skeleton";
        card.setAttribute("aria-hidden", "true");
        const cover = document.createElement("div");
        cover.className = "homecover";
        const title = document.createElement("span");
        title.className = "cardtitle skeleton-line";
        const artist = document.createElement("span");
        artist.className = "cardartist skeleton-line skeleton-artist";
        card.append(cover, title, artist);
        cards.appendChild(card);
      }
      block.appendChild(cards);
    } else if (data.error || !data.items.length) {
      const message = document.createElement("p");
      message.className = "sectionmessage";
      message.setAttribute("role", "status");
      message.textContent = data.error ? "Belum bisa dimuat. Coba Refresh." : "Belum ada rekomendasi tersedia.";
      block.appendChild(message);
    } else {
      const cards = document.createElement("div");
      cards.className = "homecards";
      const nav = document.createElement("div");
      nav.className = "sectionnav";
      [-1, 1].forEach((direction) => {
        const arrow = document.createElement("button");
        arrow.textContent = direction < 0 ? "‹" : "›";
        arrow.setAttribute("aria-label", `${direction < 0 ? "Sebelumnya" : "Berikutnya"}: ${section.title}`);
        arrow.addEventListener("click", () => { cards.scrollLeft += direction * cards.clientWidth; });
        nav.appendChild(arrow);
      });
      block.appendChild(nav);
      data.items.forEach((item, index) => {
        const card = document.createElement("button");
        card.className = "homecard";
        card.type = "button";
        const img = document.createElement("img");
        img.className = "homecover";
        img.alt = "";
        img.loading = "lazy";
        if (item.cover) img.src = item.cover;
        const title = document.createElement("span");
        title.className = "cardtitle";
        title.textContent = item.name;
        const artist = document.createElement("span");
        artist.className = "cardartist";
        artist.textContent = item.artist || (item.kind === "album" ? "Album" : item.kind === "playlist" ? "Playlist" : "YouTube Music");
        card.append(img, title, artist);
        card.addEventListener("click", () => {
          if (item.kind === "track") {
            const tracks = data.items.filter((t) => t.kind === "track");
            const trackIndex = tracks.findIndex((t) => t.id === item.id);
            setQueue(tracks, trackIndex !== -1 ? trackIndex : index);
            state.wantVideo = !!item.video;
            playCurrent();
          } else openCollection(item);
        });
        cards.appendChild(card);
      });
      block.appendChild(cards);
    }
    root.appendChild(block);
  });
}

async function loadHome() {
  if (state.homeLoading) return;
  state.homeLoading = true;
  state.homeData = {};
  renderHome();
  await Promise.all(homeSections.map(async ({ id }) => {
    try {
      state.homeData[id] = await invoke("home_section", { section: id });
    } catch (error) {
      state.homeData[id] = { items: [], error: String(error) };
    }
    renderHome();
  }));
  state.homeLoading = false;
  state.homeLoaded = true;
  renderHome();
}

function goHome() {
  state.viewId++;
  state.view = "home";
  state.loading = false;
  state.results = [];
  $("search").value = "";
  $("clearSearch").classList.add("hidden");
  setStatus(null);
  renderResults();
  $("main").scrollTop = 0;
  if (!state.homeLoaded) loadHome();
}

async function openCollection(item) {
  const viewId = ++state.viewId;
  state.view = "collection";
  state.results = [];
  state.loading = true;
  $("collectionTitle").textContent = item.name;
  $("search").value = "";
  $("clearSearch").classList.add("hidden");
  setStatus("Ngambil daftar lagu…", true);
  renderResults();
  $("main").scrollTop = 0;
  try {
    const results = await invoke("collection_tracks", { id: item.id, kind: item.kind });
    if (viewId !== state.viewId) return;
    state.results = results;
    setStatus(results.length ? null : "Belum ada lagu di koleksi ini.");
  } catch (error) {
    if (viewId !== state.viewId) return;
    setStatus("Koleksi belum bisa dimuat. Balik ke Beranda dan coba lagi.");
  } finally {
    if (viewId === state.viewId) {
      state.loading = false;
      renderResults();
    }
  }
}

const fmtTime = (s) => {
  if (!isFinite(s) || s < 0) s = 0;
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${String(sec).padStart(2, "0")}`;
};

let toastTimer;
function toast(msg) {
  const el = $("toast");
  el.textContent = msg;
  el.classList.remove("hidden");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.add("hidden"), 3500);
}

function setStatus(text, loading = false) {
  const el = $("status");
  el.textContent = "";
  if (!text) {
    el.classList.add("hidden");
    return;
  }
  if (loading) {
    const sp = document.createElement("span");
    sp.className = "spinner";
    el.appendChild(sp);
  }
  el.appendChild(document.createTextNode(text));
  el.classList.remove("hidden");
}

function syncPlaybackLoadingUI() {
  const loading = Boolean(state.starting || state.buffering || state.recovering);
  const player = $("player");
  player.classList.toggle("is-loading", loading);
  player.setAttribute("aria-busy", String(loading));
  $("pMetaSkeleton").classList.toggle("hidden", !loading);
  $("playLoading").classList.toggle("hidden", !loading);
}

function setPlaybackStatus(text, loading = false) {
  syncPlaybackLoadingUI();
  if (state.loading) return;
  setStatus(text, loading);
}

function coverUrl(t) {
  return t.cover || "";
}

function renderResults() {
  const ul = $("results");
  ul.textContent = "";
  $("home").classList.toggle("hidden", state.view !== "home");
  $("collectionHead").classList.toggle("hidden", state.view !== "collection");
  ul.classList.toggle("hidden", state.view === "home");
  $("empty").classList.toggle("hidden", state.view === "home" || state.results.length > 0 || state.loading || !$("status").classList.contains("hidden"));
  state.results.forEach((t, i) => {
    const li = document.createElement("li");
    li.className = "result";
    li.dataset.id = t.id;
    li.tabIndex = 0;
    li.setAttribute("role", "button");
    li.setAttribute("aria-label", `${t.name}, ${t.artist || "YouTube"}`);
    li.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        li.click();
      }
    });

    const img = document.createElement("img");
    img.className = "rcover";
    img.loading = "lazy";
    if (t.cover) img.src = t.cover;
    img.alt = "";

    const meta = document.createElement("div");
    meta.className = "rmeta";
    const title = document.createElement("div");
    title.className = "rtitle";
    title.textContent = t.name;
    const sub = document.createElement("div");
    sub.className = "rsub";
    sub.textContent = t.artist || "YouTube";
    meta.append(title, sub);

    const dur = document.createElement("span");
    dur.className = "rdur";
    dur.textContent = t.duration ? fmtTime(t.duration) : "";

    li.append(img, meta, dur);
    li.addEventListener("click", () => {
      setQueue(state.results, i);
      state.wantVideo = !!t.video;
      playCurrent();
    });
    ul.appendChild(li);
  });
  highlightPlaying();
}

function renderQueue() {
  const ul = $("queueList");
  ul.textContent = "";
  state.queue.forEach((t, i) => {
    const li = document.createElement("li");
    li.className = "qitem";
    const img = document.createElement("img");
    img.className = "rcover";
    if (t.cover) img.src = t.cover;
    img.alt = "";
    const meta = document.createElement("div");
    meta.className = "rmeta";
    const title = document.createElement("div");
    title.className = "rtitle";
    title.textContent = t.name;
    const sub = document.createElement("div");
    sub.className = "rsub";
    sub.textContent = t.artist || "YouTube";
    meta.append(title, sub);
    li.append(img, meta);
    li.addEventListener("click", () => {
      state.index = i;
      state.wantVideo = !!t.video || state.wantVideo;
      playCurrent();
    });
    ul.appendChild(li);
  });
  highlightPlaying();
}

function highlightPlaying() {
  const cur = state.queue[state.index];
  document.querySelectorAll(".result").forEach((el) => {
    el.classList.toggle("playing", !!cur && el.dataset.id === cur.id);
  });
  document.querySelectorAll(".qitem").forEach((el, i) => {
    el.classList.toggle("playing", i === state.index);
  });
}

async function doSearch() {
  const q = $("search").value.trim();
  if (!q) { goHome(); return; }
  const viewId = ++state.viewId;
  state.view = "search";
  state.results = [];
  state.loading = true;
  setStatus("Nyari…", true);
  renderResults();
  $("main").scrollTop = 0;
  try {
    const results = await invoke("search", { query: q, kind: state.kind });
    if (viewId !== state.viewId) return;
    state.results = results;
    setStatus(results.length ? null : "Nggak ada hasil. Coba kata kunci lain.");
  } catch (e) {
    if (viewId !== state.viewId) return;
    setStatus("Pencarian gagal. Coba lagi.");
    toast(`Gagal nyari: ${e}`);
  } finally {
    if (viewId === state.viewId) {
      state.loading = false;
      $("search").blur();
      renderResults();
    }
  }
}

let ytPlayer = null;
let ytReady = false;
let pendingPlay = null;

if (
  typeof window !== "undefined" &&
  typeof document !== "undefined" &&
  typeof document.getElementsByTagName === "function"
) {
  const tag = document.createElement("script");
  tag.src = "https://www.youtube.com/iframe_api";
  const scripts = document.getElementsByTagName("script");
  if (scripts.length > 0 && scripts[0].parentNode) {
    scripts[0].parentNode.insertBefore(tag, scripts[0]);
  } else if (document.head) {
    document.head.appendChild(tag);
  }
}

window.onYouTubeIframeAPIReady = function () {
  try {
    ytPlayer = new YT.Player("ytPlayer", {
      height: "100%",
      width: "100%",
      playerVars: {
        autoplay: 1,
        controls: 0,
        disablekb: 1,
        fs: 0,
        modestbranding: 1,
        playsinline: 1,
        rel: 0,
        iv_load_policy: 3,
      },
      events: {
        onReady: () => {
          ytReady = true;
          const vol = Number($("volume").value) || 80;
          try { ytPlayer.setVolume(vol); } catch (_) {}
          if (pendingPlay) {
            const { position, wantPlaying } = pendingPlay;
            pendingPlay = null;
            playCurrent(position, wantPlaying);
          }
        },
        onStateChange: (event) => {
          if (state.mode !== "yt") return;
          if (event.data === YT.PlayerState.PLAYING) {
            state.wantPlaying = true;
            state.starting = false;
            state.buffering = false;
            setPlaybackStatus(null);
            setPlayingUI(true);
          } else if (event.data === YT.PlayerState.PAUSED) {
            state.wantPlaying = false;
            state.buffering = false;
            setPlayingUI(false);
          } else if (event.data === YT.PlayerState.ENDED) {
            state.wantPlaying = false;
            state.buffering = false;
            setPlayingUI(false);
            next(true);
          } else if (event.data === YT.PlayerState.BUFFERING) {
            state.buffering = true;
            setPlaybackStatus("Buffering…", true);
          }
        },
        onError: (event) => {
          if (state.mode !== "yt") return;
          console.error("YouTube Player error:", event.data);
          state.buffering = false;
          setPlaybackStatus(null);
          setPlayingUI(false);
          toast("Lagu tidak dapat diputar. Memutar lagu berikutnya…");
          next(true);
        },
      },
    });
  } catch (e) {
    console.error("Failed to initialize YT.Player:", e);
  }
};

function activeEl() {
  return state.mode === "muxed" ? video : audio;
}

const mediaOps = new WeakMap();

function getMediaOps(el) {
  let ops = mediaOps.get(el);
  if (!ops) {
    ops = { play: 0, pause: 0, loadSeq: 0 };
    mediaOps.set(el, ops);
  }
  return ops;
}

function resetMediaOps(el) {
  const ops = getMediaOps(el);
  ops.play = 0;
  ops.pause = 0;
  ops.loadSeq++;
}

function setMediaSrc(el, url) {
  resetMediaOps(el);
  el.src = url;
}

function clearMediaSrc(el) {
  clearPendingSeek(el);
  resetMediaOps(el);
  el.removeAttribute("src");
  el.load();
}

function programmaticPause(el) {
  const ops = getMediaOps(el);
  const wasPaused = el.paused;
  ops.pause++;
  el.pause();
  if (wasPaused && ops.pause > 0) ops.pause--;
}

function programmaticPlay(el) {
  const ops = getMediaOps(el);
  const loadSeq = ops.loadSeq;
  const wasPaused = el.paused;
  ops.play++;
  let p;
  try {
    p = el.play();
  } catch (err) {
    if (ops.loadSeq === loadSeq && ops.play > 0 && el.paused) ops.play--;
    return Promise.reject(err);
  }
  if (!wasPaused && ops.play > 0) ops.play--;
  if (p && typeof p.catch === "function") {
    return p.catch((err) => {
      if (ops.loadSeq === loadSeq && wasPaused && el.paused && ops.play > 0 && err?.name !== "AbortError") {
        ops.play--;
      }
      throw err;
    });
  }
  return Promise.resolve(p);
}

const pendingSeekHandlers = new WeakMap();

function clearPendingSeek(el) {
  const prev = pendingSeekHandlers.get(el);
  if (!prev) return;
  pendingSeekHandlers.delete(el);
  if (typeof el.removeEventListener === "function") {
    el.removeEventListener("loadedmetadata", prev);
  }
}

function applyInitialPosition(el, position, playbackId, expectedSrc = el.src) {
  clearPendingSeek(el);
  const target = Number.isFinite(position) && position > 0 ? position : 0;
  if (target === 0) {
    if (el.readyState >= 1) {
      try { el.currentTime = 0; } catch (_) {}
    }
    return;
  }
  if (el.readyState >= 1) {
    const dur = el.duration;
    try {
      el.currentTime = Number.isFinite(dur) && dur > 0 ? Math.min(target, dur) : target;
    } catch (_) {}
    return;
  }
  let applied = false;
  const onMeta = () => {
    if (applied) return;
    if (pendingSeekHandlers.get(el) === onMeta) {
      pendingSeekHandlers.delete(el);
    }
    if (typeof el.removeEventListener === "function") {
      el.removeEventListener("loadedmetadata", onMeta);
    }
    if (playbackId !== state.playbackId || !state.mode) return;
    if (expectedSrc && el.src !== expectedSrc) return;
    applied = true;
    const dur = el.duration;
    try {
      el.currentTime = Number.isFinite(dur) && dur > 0 ? Math.min(target, dur) : target;
    } catch (_) {}
    onTimeUpdate();
  };
  pendingSeekHandlers.set(el, onMeta);
  el.addEventListener("loadedmetadata", onMeta, { once: true });
}

function clearRecoveryTimer() {
  if (state.recoveryTimer !== null) {
    clearTimeout(state.recoveryTimer);
    state.recoveryTimer = null;
  }
}

function stopAll() {
  clearRecoveryTimer();
  state.buffering = false;
  state.splitBufferingPaused = false;
  state.splitBufferingSeq++;
  programmaticPause(audio);
  programmaticPause(video);
  clearMediaSrc(audio);
  clearMediaSrc(video);
  if (ytPlayer && ytReady && ytPlayer.stopVideo) {
    try { ytPlayer.stopVideo(); } catch (_) {}
  }
}

function getAudioCandidates(list) {
  const supported = (list || []).filter((s) => audio.canPlayType(s.mime));
  const mp4 = supported.filter((s) => s.mime.includes("mp4"));
  const webm = supported.filter((s) => s.mime.includes("webm"));
  const others = supported.filter((s) => !s.mime.includes("webm") && !s.mime.includes("mp4"));
  return [...mp4, ...webm, ...others];
}

function pickAudio(list) {
  const candidates = getAudioCandidates(list);
  return candidates[0];
}

function getVideoCandidates(list, maxH = 720) {
  const supported = (list || []).filter((s) => video.canPlayType(s.mime));
  const mp4 = supported.filter((s) => s.mime.includes("avc1"));
  const pool = mp4.length ? mp4 : supported;
  const capped = pool.filter((s) => s.height <= maxH);
  const primary = capped.length ? capped : pool;
  const seen = new Set(primary);
  const rest = supported.filter((s) => !seen.has(s));
  return [...primary, ...rest];
}

function pickVideo(list, maxH = 720) {
  return getVideoCandidates(list, maxH)[0];
}

async function playCurrent(position = 0, wantPlaying = true) {
  const t = state.queue[state.index];
  if (!t) return;
  const playbackId = ++state.playbackId;
  clearRecoveryTimer();
  state.mode = null;
  state.starting = true;
  state.wantPlaying = wantPlaying;
  state.seeking = false;
  state.buffering = false;
  state.splitBufferingPaused = false;
  state.splitBufferingSeq++;
  state.recovering = false;
  state.audioCandidateIdx = 0;
  state.videoCandidateIdx = 0;
  state.muxedCandidateIdx = 0;
  state.streamRefreshCount = 0;
  state.pendingResumePos = Number.isFinite(position) && position > 0 ? position : 0;
  state.lastProgressPos = state.pendingResumePos;
  state.lastRecoveryAt = 0;
  state.radioPendingId = 0;
  stopAll();
  syncPlaybackLoadingUI();
  state.streams = null;

  $("pTitle").textContent = t.name;
  $("pArtist").textContent = t.artist || "YouTube";
  const cover = $("pCover");
  if (t.cover) {
    cover.src = t.cover;
    cover.classList.remove("hidden");
  } else {
    cover.classList.add("hidden");
  }
  invoke("set_tray_title", { title: `${t.name}: ${t.artist}` }).catch(() => {});
  highlightPlaying();
  renderQueue();
  updateMediaSession(t);

  const useVideo = state.wantVideo;
  if (!useVideo && typeof document !== "undefined" && document.pictureInPictureElement) {
    document.exitPictureInPicture().catch(() => {});
  }
  $("videoStage").classList.toggle("hidden", !useVideo);
  $("videoBtn").classList.remove("hidden");
  $("videoBtn").classList.toggle("on", useVideo);

  // Primary playback: Native HTML5 video/audio via Tauri streaming proxy
  await playCurrentNative(t, position, wantPlaying, playbackId);
}

async function playCurrentNative(t, position, wantPlaying, playbackId) {
  setPlaybackStatus("Ngambil stream…", true);

  try {
    const s = await invoke("streams", { videoId: t.id });
    if (playbackId !== state.playbackId) return;
    state.streams = s;
    const hasVideo = s.muxed.length > 0 || s.video_only.length > 0;
    $("videoBtn").classList.toggle("hidden", !hasVideo);
    $("videoBtn").classList.toggle("on", state.wantVideo && hasVideo);

    if (state.wantVideo && hasVideo) {
      $("videoStage").classList.remove("hidden");
      video.classList.remove("hidden");
      if ($("ytPlayer")) $("ytPlayer").classList.add("hidden");

      const muxedCandidates = getVideoCandidates(s.muxed, 720);
      const videoCandidates = getVideoCandidates(s.video_only, 720);
      const audioCandidates = getAudioCandidates(s.audio);

      let started = false;
      if (muxedCandidates.length) {
        state.mode = "muxed";
        for (let i = 0; i < muxedCandidates.length; i++) {
          if (playbackId !== state.playbackId) return;
          const muxed = muxedCandidates[i];
          state.muxedCandidateIdx = i;
          try {
            setMediaSrc(video, muxed.url);
            video.muted = false;
            applyInitialPosition(video, position, playbackId, muxed.url);
            setPlaybackStatus(null);
            setPlayingUI(state.wantPlaying);
            if (state.wantPlaying) {
              await programmaticPlay(video);
            }
            started = true;
            break;
          } catch (e) {
            if (playbackId !== state.playbackId || e.name === "AbortError") return;
            console.warn(`Muxed candidate ${muxed.mime} failed, trying next...`, e);
          }
        }
        if (!started && (!videoCandidates.length || !audioCandidates.length)) {
          if (state.wantPlaying) {
            if (video.error) await recoverPlayback("startup-error", video);
            else failPlayback("Gagal muter stream video.");
            return;
          }
        }
      }

      if (!started) {
        if (!videoCandidates.length || !audioCandidates.length) {
          throw new Error("nggak ada stream yang bisa diputer");
        }
        state.mode = "split";
        let vIdx = 0;
        let aIdx = 0;
        let lastErr = null;
        while (vIdx < videoCandidates.length && aIdx < audioCandidates.length) {
          if (playbackId !== state.playbackId) return;
          const v = videoCandidates[vIdx];
          const a = audioCandidates[aIdx];
          state.videoCandidateIdx = vIdx;
          state.audioCandidateIdx = aIdx;
          try {
            if (video.src !== v.url) {
              setMediaSrc(video, v.url);
              video.muted = true;
            }
            if (audio.src !== a.url) {
              setMediaSrc(audio, a.url);
            }
            applyInitialPosition(video, position, playbackId, v.url);
            applyInitialPosition(audio, position, playbackId, a.url);
            setPlaybackStatus(null);
            setPlayingUI(state.wantPlaying);
            if (state.wantPlaying) {
              await Promise.all([programmaticPlay(video), programmaticPlay(audio)]);
            }
            started = true;
            break;
          } catch (e) {
            if (playbackId !== state.playbackId || e.name === "AbortError") return;
            lastErr = e;
            programmaticPause(audio);
            programmaticPause(video);
            if (video.error && vIdx + 1 < videoCandidates.length) {
              vIdx++;
            } else if (aIdx + 1 < audioCandidates.length) {
              aIdx++;
            } else if (vIdx + 1 < videoCandidates.length) {
              vIdx++;
              aIdx = 0;
            } else {
              break;
            }
          }
        }
        if (!started && state.wantPlaying) {
          const failedEl = video.error ? video : audio.error ? audio : null;
          if (failedEl) await recoverPlayback("startup-error", failedEl);
          else failPlayback(`Gagal muter: ${lastErr}`);
          return;
        }
      }
    } else {
      const candidates = getAudioCandidates(s.audio);
      if (!candidates.length) throw new Error("nggak ada stream audio");
      state.mode = "audio";
      if (typeof document !== "undefined" && document.pictureInPictureElement) {
        document.exitPictureInPicture().catch(() => {});
      }
      $("videoStage").classList.add("hidden");
      video.classList.add("hidden");

      let played = false;
      for (let i = 0; i < candidates.length; i++) {
        if (playbackId !== state.playbackId) return;
        const a = candidates[i];
        state.audioCandidateIdx = i;
        try {
          setMediaSrc(audio, a.url);
          applyInitialPosition(audio, position, playbackId, a.url);
          setPlaybackStatus(null);
          setPlayingUI(state.wantPlaying);
          if (state.wantPlaying) {
            await programmaticPlay(audio);
          }
          played = true;
          break;
        } catch (e) {
          if (playbackId !== state.playbackId || e.name === "AbortError") return;
          console.warn(`Stream candidate ${a.mime} failed, trying next...`, e);
        }
      }
      if (!played && state.wantPlaying) {
        if (playbackId === state.playbackId) {
          if (audio.error) await recoverPlayback("startup-error", audio);
          else failPlayback("Gagal muter stream audio.");
        }
        return;
      }
    }
    if (playbackId !== state.playbackId) return;
    setPlaybackStatus(null);
    setPlayingUI(state.wantPlaying);
  } catch (e) {
    if (playbackId !== state.playbackId || e.name === "AbortError") return;
    console.warn("Native stream extraction failed, attempting fallback:", e);
    if (typeof YT !== "undefined" && YT.Player && ytPlayer && ytReady && ytPlayer.loadVideoById) {
      state.mode = "yt";
      if (typeof document !== "undefined" && document.pictureInPictureElement) {
        document.exitPictureInPicture().catch(() => {});
      }
      if (state.wantVideo) {
        $("videoStage").classList.remove("hidden");
        video.classList.add("hidden");
        if ($("ytPlayer")) $("ytPlayer").classList.remove("hidden");
      }
      setPlaybackStatus("Memuat lagu cadangan…", true);
      ytPlayer.loadVideoById({
        videoId: t.id,
        startSeconds: position,
      });
      if (!wantPlaying) {
        ytPlayer.pauseVideo();
      }
      return;
    }
    failPlayback(`Gagal muter: ${e}`);
  } finally {
    if (playbackId === state.playbackId) {
      state.starting = false;
      syncPlaybackLoadingUI();
    }
  }
}

function scheduleStallRecovery(el) {
  if (!state.mode || state.mode === "yt" || state.starting || !state.wantPlaying || state.recovering || state.seeking) return;
  if (state.mode !== "split" && el !== activeEl()) return;
  if (el.ended || activeEl().ended) return;
  if (state.recoveryTimer !== null) return;

  const playbackId = state.playbackId;
  const scheduledPos = activeEl().currentTime;
  state.recoveryTimer = setTimeout(() => {
    state.recoveryTimer = null;
    if (playbackId !== state.playbackId) return;
    if (!state.mode || state.mode === "yt" || state.starting || !state.wantPlaying || state.recovering || state.seeking) return;
    if (el.ended || activeEl().ended) return;

    const curPos = activeEl().currentTime;
    const progressed = Math.abs(curPos - scheduledPos) > 0.25;
    const ready = state.mode === "split"
      ? (!audio.paused && !video.paused && audio.readyState >= 3 && video.readyState >= 3)
      : (!activeEl().paused && activeEl().readyState >= 3);

    if (progressed && ready) {
      state.buffering = false;
      state.splitBufferingPaused = false;
      state.splitBufferingSeq++;
      setPlaybackStatus(null);
      return;
    }
    recoverPlayback("stall", el);
  }, STALL_TIMEOUT_MS);
}

async function recoverPlayback(reason, failedEl = null) {
  if (!state.mode || state.mode === "yt" || state.recovering) return;
  const t = state.queue[state.index];
  if (!t) return;
  const playbackId = state.playbackId;

  const cur = activeEl().currentTime;
  const resumePos = (Number.isFinite(cur) && cur > 0)
    ? cur
    : (state.pendingResumePos || state.lastProgressPos || 0);
  const seekSeqAtStart = state.seekSeq;
  state.pendingResumePos = resumePos;
  const getResumePos = () => (state.seekSeq !== seekSeqAtStart
    ? state.pendingResumePos
    : (state.pendingResumePos || resumePos));

  clearRecoveryTimer();
  state.recovering = true;
  state.buffering = true;
  state.splitBufferingPaused = false;
  state.splitBufferingSeq++;
  setPlaybackStatus("Menyambungkan ulang…", true);
  programmaticPause(audio);
  programmaticPause(video);

  const tryCurrentPool = async (failed) => {
    if (playbackId !== state.playbackId) return false;
    if (state.mode === "audio") {
      const candidates = getAudioCandidates(state.streams?.audio || []);
      for (let i = state.audioCandidateIdx + 1; i < candidates.length; i++) {
        if (playbackId !== state.playbackId) return false;
        const a = candidates[i];
        state.audioCandidateIdx = i;
        try {
          setMediaSrc(audio, a.url);
          applyInitialPosition(audio, getResumePos(), playbackId, a.url);
          if (state.wantPlaying) await programmaticPlay(audio);
          if (playbackId !== state.playbackId) return false;
          if (!audio.error) return true;
        } catch (e) {
          if (playbackId !== state.playbackId || e.name === "AbortError") throw e;
          console.warn(`Recovery audio candidate ${i} failed (${reason}):`, e);
        }
      }
      return false;
    }

    if (state.mode === "muxed") {
      const candidates = getVideoCandidates(state.streams?.muxed || [], 720);
      for (let i = state.muxedCandidateIdx + 1; i < candidates.length; i++) {
        if (playbackId !== state.playbackId) return false;
        const m = candidates[i];
        state.muxedCandidateIdx = i;
        try {
          setMediaSrc(video, m.url);
          video.muted = false;
          applyInitialPosition(video, getResumePos(), playbackId, m.url);
          if (state.wantPlaying) await programmaticPlay(video);
          if (playbackId !== state.playbackId) return false;
          if (!video.error) return true;
        } catch (e) {
          if (playbackId !== state.playbackId || e.name === "AbortError") throw e;
          console.warn(`Recovery muxed candidate ${i} failed (${reason}):`, e);
        }
      }
      return false;
    }

    if (state.mode === "split") {
      const aCandidates = getAudioCandidates(state.streams?.audio || []);
      const vCandidates = getVideoCandidates(state.streams?.video_only || [], 720);
      if (!aCandidates.length || !vCandidates.length) return false;

      const freshPool = state.audioCandidateIdx < 0 || state.videoCandidateIdx < 0;
      let audioFailed = freshPool || failed === audio || Boolean(audio.error);
      let videoFailed = freshPool || failed === video || Boolean(video.error);
      if (!audioFailed && !videoFailed) {
        audioFailed = audio.readyState < 3;
        videoFailed = video.readyState < 3;
      }
      if (!audioFailed && !videoFailed) {
        audioFailed = true;
        videoFailed = true;
      }

      const trySplitPair = async (vi, ai) => {
        if (playbackId !== state.playbackId) return false;
        const v = vCandidates[vi];
        const a = aCandidates[ai];
        state.videoCandidateIdx = vi;
        state.audioCandidateIdx = ai;
        const targetPos = getResumePos();
        try {
          if (video.src !== v.url) {
            setMediaSrc(video, v.url);
            video.muted = true;
          }
          if (audio.src !== a.url) {
            setMediaSrc(audio, a.url);
          }
          applyInitialPosition(video, targetPos, playbackId, v.url);
          applyInitialPosition(audio, targetPos, playbackId, a.url);
          if (state.wantPlaying) await Promise.all([programmaticPlay(video), programmaticPlay(audio)]);
          if (playbackId !== state.playbackId) return false;
          if (!audio.error && !video.error) return true;
          programmaticPause(audio);
          programmaticPause(video);
        } catch (e) {
          if (playbackId !== state.playbackId || e.name === "AbortError") throw e;
          programmaticPause(audio);
          programmaticPause(video);
        }
        return false;
      };

      if (audioFailed && videoFailed) {
        const startVi = Math.max(0, state.videoCandidateIdx);
        const startAi = state.audioCandidateIdx;
        const skipAttempted = !freshPool && state.videoCandidateIdx >= 0 && state.audioCandidateIdx >= 0;
        for (let vi = startVi; vi < vCandidates.length; vi++) {
          for (let ai = 0; ai < aCandidates.length; ai++) {
            if (skipAttempted && vi === startVi && ai <= startAi) continue;
            if (await trySplitPair(vi, ai)) return true;
          }
        }
        return false;
      }

      if (audioFailed) {
        const vi = Math.max(0, state.videoCandidateIdx);
        for (let ai = state.audioCandidateIdx + 1; ai < aCandidates.length; ai++) {
          if (await trySplitPair(vi, ai)) return true;
        }
        return false;
      }

      if (videoFailed) {
        const ai = Math.max(0, state.audioCandidateIdx);
        for (let vi = state.videoCandidateIdx + 1; vi < vCandidates.length; vi++) {
          if (await trySplitPair(vi, ai)) return true;
        }
      }
      return false;
    }

    return false;
  };

  try {
    let recovered = await tryCurrentPool(failedEl);

    while (!recovered && state.streamRefreshCount < MAX_STREAM_REFRESHES) {
      if (playbackId !== state.playbackId) return;
      state.streamRefreshCount++;
      state.lastRecoveryAt = Date.now();

      let s;
      try {
        s = await invoke("streams", { videoId: t.id });
      } catch (e) {
        if (playbackId !== state.playbackId) return;
        console.warn(`Recovery stream refresh ${state.streamRefreshCount} failed:`, e);
        continue;
      }
      if (playbackId !== state.playbackId) return;
      if (!s) continue;

      state.streams = s;
      state.audioCandidateIdx = -1;
      state.videoCandidateIdx = -1;
      state.muxedCandidateIdx = -1;

      const mCandidates = getVideoCandidates(s.muxed, 720);
      const vCandidates = getVideoCandidates(s.video_only, 720);
      const aCandidates = getAudioCandidates(s.audio);
      const hasVideo = (s.muxed?.length > 0) || (s.video_only?.length > 0);

      if (state.wantVideo && hasVideo) {
        $("videoStage").classList.remove("hidden");
        video.classList.remove("hidden");
        if ($("ytPlayer")) $("ytPlayer").classList.add("hidden");

        if (mCandidates.length > 0) {
          state.mode = "muxed";
          if (await tryCurrentPool(null)) {
            recovered = true;
            break;
          }
        }
        if (vCandidates.length > 0 && aCandidates.length > 0) {
          state.mode = "split";
          state.videoCandidateIdx = -1;
          state.audioCandidateIdx = -1;
          if (await tryCurrentPool(null)) {
            recovered = true;
            break;
          }
        }
      }
      if (!recovered && aCandidates.length > 0) {
        state.mode = "audio";
        state.audioCandidateIdx = -1;
        clearMediaSrc(video);
        if (typeof document !== "undefined" && document.pictureInPictureElement) {
          document.exitPictureInPicture().catch(() => {});
        }
        $("videoStage").classList.add("hidden");
        video.classList.add("hidden");
        if (await tryCurrentPool(null)) {
          recovered = true;
          break;
        }
      }
    }

    if (playbackId !== state.playbackId) return;
    if (recovered) {
      state.buffering = false;
      state.splitBufferingPaused = false;
      state.splitBufferingSeq++;
      setPlaybackStatus(null);
      setPlayingUI(state.wantPlaying);
      if (!state.lastRecoveryAt) state.lastRecoveryAt = Date.now();
    } else {
      failPlayback("Stream terputus. Tekan play untuk coba lagi, atau pilih lagu lain.");
    }
  } catch (e) {
    if (playbackId === state.playbackId && e && e.name === "AbortError") {
      state.buffering = false;
      state.splitBufferingPaused = false;
      state.splitBufferingSeq++;
      setPlaybackStatus(null);
    }
  } finally {
    if (playbackId === state.playbackId) {
      state.recovering = false;
      syncPlaybackLoadingUI();
    }
  }
}

function failPlayback(message) {
  state.wantPlaying = false;
  state.mode = null;
  state.recovering = false;
  state.buffering = false;
  state.splitBufferingPaused = false;
  state.splitBufferingSeq++;
  state.pendingResumePos = 0;
  stopAll();
  setPlaybackStatus(null);
  setPlayingUI(false);
  toast(message);
}

function setPlayingUI(playing) {
  $("iconPlay").classList.toggle("hidden", playing);
  $("iconPause").classList.toggle("hidden", !playing);
  syncPlaybackLoadingUI();
}

async function togglePlay() {
  if (state.mode === "yt" && ytPlayer && ytReady) {
    state.wantPlaying = !state.wantPlaying;
    state.buffering = state.wantPlaying;
    if (state.wantPlaying) {
      try { ytPlayer.playVideo(); } catch (_) {}
    } else {
      try { ytPlayer.pauseVideo(); } catch (_) {}
    }
    setPlayingUI(state.wantPlaying);
    return;
  }
  if (!state.mode) {
    if (state.starting) {
      state.playbackId++;
      state.starting = false;
      state.wantPlaying = false;
      stopAll();
      setPlaybackStatus(null);
      setPlayingUI(false);
      return;
    }
    if (state.queue.length) await playCurrent();
    return;
  }
  state.wantPlaying = !state.wantPlaying;
  state.splitBufferingPaused = false;
  state.splitBufferingSeq++;
  if (!state.wantPlaying) {
    clearRecoveryTimer();
    state.buffering = false;
    setPlaybackStatus(null);
  }
  const els = state.mode === "split" ? [audio, video] : [activeEl()];
  const playbackId = state.playbackId;
  state.buffering = state.wantPlaying;
  syncPlaybackLoadingUI();
  try {
    if (!state.wantPlaying) els.forEach((el) => programmaticPause(el));
    else await Promise.all(els.map((el) => programmaticPlay(el)));
    if (playbackId === state.playbackId) setPlayingUI(state.wantPlaying);
  } catch (e) {
    if (playbackId === state.playbackId && e.name !== "AbortError") {
      recoverPlayback("play-error", activeEl());
    }
  }
}

async function next(auto = false) {
  if (!state.queue.length) return;
  if (auto && state.repeat === "one") {
    playCurrent(0, true);
    return;
  }
  if (state.index < state.queue.length - 1) {
    state.index++;
    playCurrent();
    return;
  }
  if (state.repeat === "all") {
    state.index = 0;
    playCurrent();
    return;
  }
  if (state.radioPendingId) return;
  const last = state.queue[state.queue.length - 1];
  const playbackId = state.playbackId;
  const queueLen = state.queue.length;
  const indexSnapshot = state.index;
  const radioId = ++state.radioSeq;
  state.radioPendingId = radioId;
  try {
    const more = await invoke("radio", { videoId: last.id });
    if (
      radioId !== state.radioPendingId ||
      playbackId !== state.playbackId ||
      state.queue.length !== queueLen ||
      state.index !== indexSnapshot ||
      state.queue[state.index]?.id !== last.id
    ) {
      return;
    }
    const known = new Set(state.queue.map((t) => t.id));
    const fresh = more.filter((t) => !known.has(t.id));
    if (!fresh.length) {
      if (auto) setPlayingUI(false);
      return;
    }
    state.queue.push(...fresh);
    if (state.originalQueue) state.originalQueue.push(...fresh);
    state.index++;
    if (auto) toast("Radio: lanjut otomatis ♪");
    state.radioPendingId = 0;
    playCurrent();
  } catch (e) {
    if (radioId !== state.radioPendingId || playbackId !== state.playbackId) return;
    if (auto) {
      setPlayingUI(false);
      toast(`Radio gagal: ${e}`);
    }
  } finally {
    if (state.radioPendingId === radioId) state.radioPendingId = 0;
  }
}

function prev() {
  if (state.mode === "yt" && ytPlayer && ytReady && ytPlayer.getCurrentTime) {
    let cur = 0;
    try { cur = ytPlayer.getCurrentTime() || 0; } catch (_) {}
    if (cur > 3) {
      seekTo(0);
      return;
    }
    if (state.index > 0) {
      state.index--;
      playCurrent();
    } else if (state.repeat === "all" && state.queue.length > 0) {
      state.index = state.queue.length - 1;
      playCurrent();
    } else {
      seekTo(0);
    }
    return;
  }
  const el = activeEl();
  if (state.mode && el.currentTime > 3) {
    seekTo(0);
    return;
  }
  if (state.index > 0) {
    state.index--;
    playCurrent();
  } else if (state.repeat === "all" && state.queue.length > 0) {
    state.index = state.queue.length - 1;
    playCurrent();
  } else {
    seekTo(0);
  }
}

function seekTo(sec) {
  if (state.mode === "yt" && ytPlayer && ytReady && ytPlayer.seekTo) {
    let dur = 0;
    try { dur = ytPlayer.getDuration() || 0; } catch (_) {}
    const time = Math.max(0, Math.min(sec, Number.isFinite(dur) && dur > 0 ? dur : sec));
    try { ytPlayer.seekTo(time, true); } catch (_) {}
    return;
  }
  if (!state.mode) return;
  state.seekSeq++;
  clearRecoveryTimer();
  const wasBuffering = state.buffering;
  const wasSplitBufferingPaused = state.splitBufferingPaused;
  if (!state.recovering) {
    state.buffering = false;
    state.splitBufferingPaused = false;
    state.splitBufferingSeq++;
    if (wasBuffering) setPlaybackStatus(null);
  }
  const duration = activeEl().duration;
  const time = Math.max(0, Math.min(sec, Number.isFinite(duration) && duration > 0 ? duration : sec));
  state.pendingResumePos = time;
  state.lastProgressPos = time;
  applyInitialPosition(activeEl(), time, state.playbackId);
  if (state.mode === "split") applyInitialPosition(video, time, state.playbackId);
  if (!state.recovering && wasSplitBufferingPaused && state.wantPlaying && state.mode === "split") {
    if (audio.readyState >= 3 && video.readyState >= 3) {
      const playbackId = state.playbackId;
      Promise.all([programmaticPlay(audio), programmaticPlay(video)]).catch((e) => {
        if (playbackId === state.playbackId && e.name !== "AbortError") {
          recoverPlayback("play-error", null);
        }
      });
    } else {
      state.buffering = true;
      state.splitBufferingPaused = true;
      state.splitBufferingSeq++;
      setPlaybackStatus("Buffering…", true);
      scheduleStallRecovery(audio.readyState < 3 ? audio : video);
    }
  }
}

function onTimeUpdate(event) {
  if (event?.target && event.target !== activeEl()) return;
  if (state.seeking) return;
  const el = activeEl();
  const dur = el.duration || 0;
  $("curTime").textContent = fmtTime(el.currentTime);
  $("durTime").textContent = fmtTime(dur);
  if (dur > 0) $("progress").value = Math.round((el.currentTime / dur) * 1000);

  if (state.mode && state.mode !== "yt" && !el.paused && el.readyState >= 3) {
    const bothReady = state.mode !== "split" || (audio.readyState >= 3 && video.readyState >= 3);
    if (bothReady) {
      if (state.buffering && !state.starting && !state.recovering) {
        state.buffering = false;
        setPlaybackStatus(null);
      }
      if (Math.abs(el.currentTime - state.lastProgressPos) > 0.25) {
        clearRecoveryTimer();
      }
      if (state.streamRefreshCount > 0 && state.lastRecoveryAt && (Date.now() - state.lastRecoveryAt >= SUSTAINED_PLAYBACK_MS)) {
        state.streamRefreshCount = 0;
        state.lastRecoveryAt = 0;
      }
      state.lastProgressPos = el.currentTime;
    }
    if (state.pendingResumePos > 0 && el.currentTime >= state.pendingResumePos - 1) {
      state.pendingResumePos = 0;
    }
  }
}

function onEnded(event) {
  if (!state.mode || event.target !== activeEl()) return;
  clearRecoveryTimer();
  state.buffering = false;
  state.splitBufferingPaused = false;
  state.splitBufferingSeq++;
  state.pendingResumePos = 0;
  state.wantPlaying = false;
  next(true);
}

function onError(event) {
  const el = event?.target || activeEl();
  if (!state.mode || state.mode === "yt") return;
  if (state.starting && state.wantPlaying) return;
  if (state.mode !== "split" && el !== activeEl()) return;
  if (!el.src) return;
  if (state.recovering) return;
  if (state.starting && !state.wantPlaying) {
    state.starting = false;
  }
  recoverPlayback("error", el);
}

function updateMediaSession(t) {
  if (!("mediaSession" in navigator)) return;
  navigator.mediaSession.metadata = new MediaMetadata({
    title: t.name,
    artist: t.artist || "YouTube",
    artwork: t.cover ? [{ src: t.cover, sizes: "512x512", type: "image/jpeg" }] : [],
  });
  navigator.mediaSession.setActionHandler("play", () => togglePlay());
  navigator.mediaSession.setActionHandler("pause", () => togglePlay());
  navigator.mediaSession.setActionHandler("previoustrack", () => prev());
  navigator.mediaSession.setActionHandler("nexttrack", () => next());
}

// --- wiring ---

$("homeBtn").addEventListener("click", goHome);
$("backHome").addEventListener("click", goHome);
$("refreshHome").addEventListener("click", loadHome);
$("clearSearch").addEventListener("click", goHome);
$("search").addEventListener("input", () => {
  const hasQuery = !!$("search").value.trim();
  $("clearSearch").classList.toggle("hidden", !hasQuery);
  if (!hasQuery) goHome();
});
$("searchBtn").addEventListener("click", doSearch);
$("search").addEventListener("keydown", (e) => {
  if (e.key === "Enter") doSearch();
});

$("tabTracks").addEventListener("click", () => switchTab("tracks"));
$("tabVideos").addEventListener("click", () => switchTab("videos"));
function switchTab(kind) {
  state.kind = kind;
  $("tabTracks").classList.toggle("active", kind === "tracks");
  $("tabVideos").classList.toggle("active", kind === "videos");
  if ($("search").value.trim()) doSearch();
}

$("playBtn").addEventListener("click", togglePlay);
$("nextBtn").addEventListener("click", () => next());
$("prevBtn").addEventListener("click", prev);
$("repeatBtn")?.addEventListener("click", toggleRepeat);
$("shuffleBtn")?.addEventListener("click", toggleShuffle);

$("videoBtn").addEventListener("click", () => {
  state.wantVideo = !state.wantVideo;
  if (!state.wantVideo && typeof document !== "undefined" && document.pictureInPictureElement) {
    document.exitPictureInPicture().catch(() => {});
  }
  $("videoStage").classList.toggle("hidden", !state.wantVideo);
  $("videoBtn").classList.toggle("on", state.wantVideo);
  const position = state.mode === "yt" && ytPlayer && ytReady && ytPlayer.getCurrentTime
    ? (ytPlayer.getCurrentTime() || 0)
    : activeEl().currentTime;
  const wantPlaying = state.wantPlaying;
  if (state.queue[state.index]) playCurrent(position, wantPlaying);
});

if ($("pipBtn")) {
  $("pipBtn").addEventListener("click", async (e) => {
    e.stopPropagation();
    try {
      if (document.pictureInPictureElement) {
        await document.exitPictureInPicture();
      } else if (video && document.pictureInPictureEnabled) {
        await video.requestPictureInPicture();
      }
    } catch (err) {
      console.warn("PiP toggle error:", err);
    }
  });
}

video.addEventListener("enterpictureinpicture", () => {
  $("pipBtn")?.classList.add("active");
});
video.addEventListener("leavepictureinpicture", () => {
  $("pipBtn")?.classList.remove("active");
});

$("queueBtn").addEventListener("click", () => {
  $("queuePanel").classList.toggle("hidden");
  renderQueue();
});
$("queueClose").addEventListener("click", () => $("queuePanel").classList.add("hidden"));

$("progress").addEventListener("input", () => {
  state.seeking = true;
  let dur = 0;
  if (state.mode === "yt" && ytPlayer && ytReady && ytPlayer.getDuration) {
    dur = ytPlayer.getDuration() || 0;
  } else {
    dur = activeEl().duration || 0;
  }
  if (dur > 0) $("curTime").textContent = fmtTime((dur * $("progress").value) / 1000);
});
$("progress").addEventListener("change", () => {
  state.seeking = false;
  let dur = 0;
  if (state.mode === "yt" && ytPlayer && ytReady && ytPlayer.getDuration) {
    dur = ytPlayer.getDuration() || 0;
  } else {
    dur = activeEl().duration || 0;
  }
  if (dur > 0) seekTo((dur * $("progress").value) / 1000);
});
const clearSeeking = () => { state.seeking = false; };
["pointerup", "mouseup", "pointercancel", "blur"].forEach((evt) => {
  $("progress").addEventListener(evt, clearSeeking);
});
if (typeof window !== "undefined" && typeof window.addEventListener === "function") {
  window.addEventListener("blur", clearSeeking);
  window.addEventListener("pointerup", clearSeeking);
}

const savedVol = getStorage("nada.volume");
$("volume").value = savedVol !== null ? savedVol : 80;
audio.volume = $("volume").value / 100;
video.volume = $("volume").value / 100;
$("volume").addEventListener("input", () => {
  const val = Number($("volume").value);
  if (ytPlayer && ytReady && ytPlayer.setVolume) {
    try { ytPlayer.setVolume(val); } catch (_) {}
  }
  const v = val / 100;
  audio.volume = v;
  video.volume = v;
  setStorage("nada.volume", $("volume").value);
});

setInterval(() => {
  if (state.seeking) return;
  if (state.mode === "yt" && ytPlayer && ytReady && ytPlayer.getCurrentTime && ytPlayer.getDuration) {
    let cur = 0;
    let dur = 0;
    try {
      cur = ytPlayer.getCurrentTime() || 0;
      dur = ytPlayer.getDuration() || 0;
    } catch (_) {}
    $("curTime").textContent = fmtTime(cur);
    $("durTime").textContent = fmtTime(dur);
    if (dur > 0) $("progress").value = Math.round((cur / dur) * 1000);
  }
}, 250);

[audio, video].forEach((el) => {
  el.addEventListener("timeupdate", onTimeUpdate);
  el.addEventListener("ended", onEnded);
  el.addEventListener("error", onError);
  el.addEventListener("seeked", () => { state.seeking = false; });
  el.addEventListener("play", () => {
    const ops = getMediaOps(el);
    if (ops.play > 0) {
      ops.play--;
      return;
    }
    if (el.paused) return;
    if (!state.mode || state.mode === "yt" || state.starting || state.splitBufferingPaused || state.recovering) return;
    if (state.mode === "split") {
      if (el === video && !state.wantPlaying && audio.paused) {
        state.wantPlaying = true;
        setPlayingUI(true);
        const playbackId = state.playbackId;
        programmaticPlay(audio).catch((e) => {
          if (playbackId === state.playbackId && e.name !== "AbortError") {
            recoverPlayback("play-error", audio);
          }
        });
        return;
      }
      if (el === audio && !state.wantPlaying && video.paused) {
        state.wantPlaying = true;
        setPlayingUI(true);
        const playbackId = state.playbackId;
        programmaticPlay(video).catch((e) => {
          if (playbackId === state.playbackId && e.name !== "AbortError") {
            recoverPlayback("play-error", video);
          }
        });
        return;
      }
      if (el === audio) setPlayingUI(true);
      return;
    }
    if (el === activeEl()) {
      state.wantPlaying = true;
      setPlayingUI(true);
    }
  });
  el.addEventListener("playing", () => {
    if (!state.mode || state.mode === "yt") return;
    if (state.mode !== "split" && el !== activeEl()) return;
    if (state.mode === "split" && (audio.readyState < 3 || video.readyState < 3)) return;
    if (el.readyState < 3) return;
    clearRecoveryTimer();
    state.buffering = false;
    state.splitBufferingPaused = false;
    state.splitBufferingSeq++;
    if (!state.starting && !state.recovering) setPlaybackStatus(null);
    setPlayingUI(true);
  });
  el.addEventListener("pause", () => {
    const ops = getMediaOps(el);
    if (ops.pause > 0) {
      ops.pause--;
      return;
    }
    if (!el.paused) return;
    if (!state.mode || state.mode === "yt" || state.starting || state.splitBufferingPaused || state.recovering || el.ended) return;
    if (state.mode === "split") {
      if (el === video) {
        if (state.wantPlaying && !audio.paused) {
          state.wantPlaying = false;
          clearRecoveryTimer();
          state.buffering = false;
          setPlayingUI(false);
          programmaticPause(audio);
        }
        return;
      }
      if (el === audio) {
        if (state.wantPlaying && !video.paused) {
          state.wantPlaying = false;
          clearRecoveryTimer();
          state.buffering = false;
          programmaticPause(video);
        }
        setPlayingUI(false);
        return;
      }
      return;
    }
    if (el === activeEl()) {
      state.wantPlaying = false;
      clearRecoveryTimer();
      state.buffering = false;
      setPlayingUI(false);
    }
  });
});

[audio, video].forEach((el) => {
  el.addEventListener("waiting", () => {
    if (!state.mode || state.mode === "yt" || state.starting || !state.wantPlaying || state.recovering || state.seeking) return;
    if (state.mode !== "split" && el !== activeEl()) return;
    if (el.ended || activeEl().ended) return;
    state.buffering = true;
    if (state.mode === "split") {
      state.splitBufferingPaused = true;
      state.splitBufferingSeq++;
      programmaticPause(audio);
      programmaticPause(video);
    }
    setPlaybackStatus("Buffering…", true);
    scheduleStallRecovery(el);
  });
  el.addEventListener("stalled", () => {
    if (!state.mode || state.mode === "yt" || state.starting || !state.wantPlaying || state.recovering || state.seeking) return;
    if (state.mode !== "split" && el !== activeEl()) return;
    if (el.ended || activeEl().ended) return;
    if (el.readyState < 3) {
      state.buffering = true;
      setPlaybackStatus("Buffering…", true);
    }
    scheduleStallRecovery(el);
  });
  el.addEventListener("canplay", async () => {
    if (!state.mode || state.mode === "yt" || state.starting || !state.wantPlaying || state.recovering) return;
    if (state.mode !== "split") {
      if (el !== activeEl() || el.readyState < 3) return;
      clearRecoveryTimer();
      state.buffering = false;
      setPlaybackStatus(null);
      return;
    }
    if (audio.readyState < 3 || video.readyState < 3) return;
    clearRecoveryTimer();
    state.buffering = false;
    const playbackId = state.playbackId;
    const bufSeq = state.splitBufferingSeq;
    try {
      await Promise.all([programmaticPlay(audio), programmaticPlay(video)]);
      if (playbackId !== state.playbackId || bufSeq !== state.splitBufferingSeq) return;
      state.splitBufferingPaused = false;
      state.splitBufferingSeq++;
      setPlaybackStatus(null);
    } catch (e) {
      if (playbackId === state.playbackId && bufSeq === state.splitBufferingSeq && e?.name !== "AbortError") {
        state.splitBufferingPaused = false;
        state.splitBufferingSeq++;
        recoverPlayback("play-error", null);
      }
    }
  });
});

setInterval(() => {
  if (state.mode !== "split" || audio.paused || video.paused) return;
  if (audio.readyState < 3 || video.readyState < 3 || audio.seeking || video.seeking) return;
  if (Math.abs(video.currentTime - audio.currentTime) > 0.3) {
    video.currentTime = audio.currentTime;
  }
}, 500);

document.addEventListener("keydown", (e) => {
  if (e.target.closest("input, button, [role='button']")) return;
  if (e.code === "Space") {
    e.preventDefault();
    togglePlay();
  } else if (e.code === "ArrowRight") {
    seekTo(activeEl().currentTime + 5);
  } else if (e.code === "ArrowLeft") {
    seekTo(activeEl().currentTime - 5);
  } else if (e.key === "r" || e.key === "R") {
    toggleRepeat();
  } else if (e.key === "s" || e.key === "S") {
    toggleShuffle();
  }
});

listen("nada://toggle-play", () => togglePlay());

video.addEventListener("click", togglePlay);
syncPlaybackLoadingUI();
updateModeButtonsUI();
renderHome();
loadHome();