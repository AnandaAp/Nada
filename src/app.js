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

const state = {
  kind: "tracks",
  results: [],
  queue: [],
  index: -1,
  mode: null, // 'audio' | 'muxed' | 'split'
  wantVideo: false,
  streams: null,
  loading: false,
  playbackId: 0,
  starting: false,
  wantPlaying: false,
  seeking: false,
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
    if (!data || data.error || !data.items.length) {
      const message = document.createElement("p");
      message.className = "sectionmessage";
      message.setAttribute("role", "status");
      message.textContent = !data ? "Lagi ngambil rekomendasi…" : data.error ? "Belum bisa dimuat. Coba Refresh." : "Belum ada rekomendasi tersedia.";
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
            state.wantVideo = item.video;
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
      state.wantVideo = t.video || state.wantVideo;
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
      state.wantVideo = t.video || state.wantVideo;
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
          if (event.data === YT.PlayerState.PLAYING) {
            state.wantPlaying = true;
            state.starting = false;
            setStatus(null);
            setPlayingUI(true);
          } else if (event.data === YT.PlayerState.PAUSED) {
            state.wantPlaying = false;
            setPlayingUI(false);
          } else if (event.data === YT.PlayerState.ENDED) {
            state.wantPlaying = false;
            setPlayingUI(false);
            next(true);
          } else if (event.data === YT.PlayerState.BUFFERING) {
            setStatus("Buffering…", true);
          }
        },
        onError: (event) => {
          console.error("YouTube Player error:", event.data);
          setStatus(null);
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

function stopAll() {
  audio.pause();
  video.pause();
  audio.removeAttribute("src");
  video.removeAttribute("src");
  audio.load();
  video.load();
  if (ytPlayer && ytReady && ytPlayer.stopVideo) {
    try { ytPlayer.stopVideo(); } catch (_) {}
  }
}

function getAudioCandidates(list) {
  const supported = list.filter((s) => audio.canPlayType(s.mime));
  const mp4 = supported.filter((s) => s.mime.includes("mp4"));
  const webm = supported.filter((s) => s.mime.includes("webm"));
  const others = supported.filter((s) => !s.mime.includes("webm") && !s.mime.includes("mp4"));
  return [...mp4, ...webm, ...others];
}

function pickAudio(list) {
  const candidates = getAudioCandidates(list);
  return candidates[0];
}

function pickVideo(list, maxH) {
  const supported = list.filter((s) => video.canPlayType(s.mime));
  const mp4 = supported.filter((s) => s.mime.includes("avc1"));
  const pool = mp4.length ? mp4 : supported;
  const capped = pool.filter((s) => s.height <= maxH);
  return (capped.length ? capped : pool)[0];
}

async function playCurrent(position = 0, wantPlaying = true) {
  const t = state.queue[state.index];
  if (!t) return;
  const playbackId = ++state.playbackId;
  state.mode = null;
  state.starting = true;
  state.wantPlaying = wantPlaying;
  stopAll();
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
  $("videoStage").classList.toggle("hidden", !useVideo);
  $("videoBtn").classList.remove("hidden");
  $("videoBtn").classList.toggle("on", useVideo);

  // Primary playback: Native HTML5 video/audio via Tauri streaming proxy
  await playCurrentNative(t, position, wantPlaying, playbackId);
}

async function playCurrentNative(t, position, wantPlaying, playbackId) {
  setStatus("Ngambil stream…", true);

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

      const muxed = pickVideo(s.muxed, 720);
      if (muxed) {
        state.mode = "muxed";
        video.src = muxed.url;
        video.muted = false;
        video.currentTime = position;
        setStatus(null);
        setPlayingUI(state.wantPlaying);
        if (state.wantPlaying) {
          try {
            await video.play();
          } catch (e) {
            if (playbackId === state.playbackId && e.name !== "AbortError") failPlayback(`Gagal muter: ${e}`);
          }
        }
      } else {
        const v = pickVideo(s.video_only, 720);
        const a = pickAudio(s.audio);
        if (!v || !a) throw new Error("nggak ada stream yang bisa diputer");
        state.mode = "split";
        video.src = v.url;
        video.muted = true;
        audio.src = a.url;
        video.currentTime = position;
        audio.currentTime = position;
        setStatus(null);
        setPlayingUI(state.wantPlaying);
        if (state.wantPlaying) {
          try {
            await Promise.all([video.play(), audio.play()]);
          } catch (e) {
            if (playbackId === state.playbackId && e.name !== "AbortError") failPlayback(`Gagal muter: ${e}`);
          }
        }
      }
    } else {
      const candidates = getAudioCandidates(s.audio);
      if (!candidates.length) throw new Error("nggak ada stream audio");
      state.mode = "audio";
      $("videoStage").classList.add("hidden");
      video.classList.add("hidden");

      let played = false;
      for (const a of candidates) {
        if (playbackId !== state.playbackId) return;
        try {
          audio.src = a.url;
          audio.currentTime = position;
          setStatus(null);
          setPlayingUI(state.wantPlaying);
          if (state.wantPlaying) {
            await audio.play();
          }
          played = true;
          break;
        } catch (e) {
          if (playbackId !== state.playbackId || e.name === "AbortError") return;
          console.warn(`Stream candidate ${a.mime} failed, trying next...`, e);
        }
      }
      if (!played && state.wantPlaying) {
        if (playbackId === state.playbackId) failPlayback("Gagal muter stream audio.");
        return;
      }
    }
    if (playbackId !== state.playbackId) return;
    setStatus(null);
    setPlayingUI(state.wantPlaying);
  } catch (e) {
    if (playbackId !== state.playbackId || e.name === "AbortError") return;
    console.warn("Native stream extraction failed, attempting fallback:", e);
    if (typeof YT !== "undefined" && YT.Player && ytPlayer && ytReady && ytPlayer.loadVideoById) {
      state.mode = "yt";
      if (state.wantVideo) {
        $("videoStage").classList.remove("hidden");
        video.classList.add("hidden");
        if ($("ytPlayer")) $("ytPlayer").classList.remove("hidden");
      }
      setStatus("Memuat lagu cadangan…", true);
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
    if (playbackId === state.playbackId) state.starting = false;
  }
}

function failPlayback(message) {
  state.wantPlaying = false;
  state.mode = null;
  stopAll();
  setStatus(null);
  setPlayingUI(false);
  toast(message);
}

function setPlayingUI(playing) {
  $("iconPlay").classList.toggle("hidden", playing);
  $("iconPause").classList.toggle("hidden", !playing);
}

async function togglePlay() {
  if (state.mode === "yt" && ytPlayer && ytReady) {
    state.wantPlaying = !state.wantPlaying;
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
      setStatus(null);
      setPlayingUI(false);
      return;
    }
    if (state.queue.length) await playCurrent();
    return;
  }
  state.wantPlaying = !state.wantPlaying;
  const els = state.mode === "split" ? [audio, video] : [activeEl()];
  const playbackId = state.playbackId;
  try {
    if (!state.wantPlaying) els.forEach((el) => el.pause());
    else await Promise.all(els.map((el) => el.play()));
    if (playbackId === state.playbackId) setPlayingUI(state.wantPlaying);
  } catch (e) {
    if (playbackId === state.playbackId && e.name !== "AbortError") failPlayback(`Gagal muter: ${e}`);
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
  const last = state.queue[state.queue.length - 1];
  const playbackId = state.playbackId;
  try {
    const more = await invoke("radio", { videoId: last.id });
    if (playbackId !== state.playbackId) return;
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
    playCurrent();
  } catch (e) {
    if (auto) {
      setPlayingUI(false);
      toast(`Radio gagal: ${e}`);
    }
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
  const duration = activeEl().duration;
  const time = Math.max(0, Math.min(sec, Number.isFinite(duration) ? duration : sec));
  activeEl().currentTime = time;
  if (state.mode === "split") video.currentTime = time;
}

function onTimeUpdate() {
  if (state.seeking) return;
  const el = activeEl();
  const dur = el.duration || 0;
  $("curTime").textContent = fmtTime(el.currentTime);
  $("durTime").textContent = fmtTime(dur);
  if (dur > 0) $("progress").value = Math.round((el.currentTime / dur) * 1000);
}

function onEnded(event) {
  if (!state.mode || event.target !== activeEl()) return;
  state.wantPlaying = false;
  next(true);
}

function onError() {
  if (!state.mode || state.starting) return;
  failPlayback("Stream terputus. Tekan play untuk coba lagi, atau pilih lagu lain.");
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
  let dur = 0;
  if (state.mode === "yt" && ytPlayer && ytReady && ytPlayer.getDuration) {
    dur = ytPlayer.getDuration() || 0;
  } else {
    dur = activeEl().duration || 0;
  }
  if (dur > 0) seekTo((dur * $("progress").value) / 1000);
  state.seeking = false;
});

const savedVol = localStorage.getItem("nada.volume");
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
  localStorage.setItem("nada.volume", $("volume").value);
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
  el.addEventListener("play", () => setPlayingUI(true));
  el.addEventListener("pause", () => {
    if (state.mode === "split" && el === video) return;
    setPlayingUI(false);
  });
});

[audio, video].forEach((el) => {
  el.addEventListener("waiting", () => {
    if (state.mode !== "split" || state.starting || !state.wantPlaying) return;
    audio.pause();
    video.pause();
    setStatus("Buffering…", true);
  });
  el.addEventListener("canplay", async () => {
    if (state.mode !== "split" || state.starting || !state.wantPlaying) return;
    if (audio.readyState < 3 || video.readyState < 3) return;
    const playbackId = state.playbackId;
    try {
      await Promise.all([audio.play(), video.play()]);
      if (playbackId === state.playbackId) setStatus(null);
    } catch (e) {
      if (playbackId === state.playbackId && e.name !== "AbortError") failPlayback(`Gagal muter: ${e}`);
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
updateModeButtonsUI();
renderHome();
loadHome();