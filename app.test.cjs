const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function player(invoke = async () => {}, homeInvoke = async () => ({ items: [] }), opts = {}) {
  const elements = new Map();
  const windowHandlers = {};
  const timers = new Map();
  let nextTimerId = 1;

  const documentObj = {
    pictureInPictureEnabled: true,
    pictureInPictureElement: null,
    getElementById(id) { if (!elements.has(id)) elements.set(id, new Element(id)); return elements.get(id); },
    createElement() { return new Element(''); },
    createTextNode(text) { return text; },
    querySelectorAll() { return []; },
    addEventListener() {},
    async exitPictureInPicture() {
      const prev = documentObj.pictureInPictureElement;
      documentObj.pictureInPictureElement = null;
      if (prev) prev.emit('leavepictureinpicture');
    },
  };

  const windowObj = {
    __TAURI__: {
      core: { invoke: (command, args) => command === 'home_section' ? homeInvoke(args) : invoke(command, args) },
      event: { listen() {} },
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

  class Element {
    constructor(id) {
      this.id = id;
      this.paused = true;
      this.currentTime = 0;
      this.duration = 200;
      this.readyState = opts.resetReadyStateOnSrc ? 0 : 4;
      this.handlers = {};
      this.children = [];
      this.dataset = {};
      this.value = id === 'search' ? '' : 80;
      this.ended = false;
      this.error = null;
      this._src = undefined;
      this._pendingMediaTasks = new Set();
      this._pendingPlays = new Set();
      const cancelPendingMediaTasks = (reason = 'The play() request was interrupted by a new load request.') => {
        for (const id of this._pendingMediaTasks) {
          globalThis.clearTimeout(id);
        }
        this._pendingMediaTasks.clear();
        if (this._pendingPlays.size) {
          const err = Object.assign(new Error(reason), { name: 'AbortError' });
          for (const reject of [...this._pendingPlays]) {
            this._pendingPlays.delete(reject);
            reject(err);
          }
        }
      };
      this._cancelPendingMediaTasks = cancelPendingMediaTasks;
      if (opts.resetReadyStateOnSrc || opts.chromiumLoadSemantics) {
        Object.defineProperty(this, 'src', {
          configurable: true,
          enumerable: true,
          get() { return this._src; },
          set(val) {
            if (opts.chromiumLoadSemantics) cancelPendingMediaTasks();
            this._src = val;
            this.readyState = 0;
            this.currentTime = 0;
            this.error = null;
            this.ended = false;
          },
        });
      }
      const classes = new Set();
      this.classList = {
        add(name) { classes.add(name); },
        remove(name) { classes.delete(name); },
        contains(name) { return classes.has(name); },
        toggle(name, force) { const add = force ?? !classes.has(name); if (add) classes.add(name); else classes.delete(name); return add; },
      };
    }
    addEventListener(type, handler, options) {
      (this.handlers[type] ??= []).push({ handler, once: Boolean(options && options.once) });
    }
    removeEventListener(type, handler) {
      if (!this.handlers[type]) return;
      this.handlers[type] = this.handlers[type].filter((entry) => entry.handler !== handler);
    }
    emit(type, extra = {}) {
      const list = [...(this.handlers[type] ?? [])];
      for (const entry of list) {
        if (entry.once) this.removeEventListener(type, entry.handler);
        entry.handler({
          target: this,
          type,
          stopPropagation() {},
          preventDefault() {},
          ...extra,
        });
      }
    }
    click() { this.emit('click'); }
    append(...items) { this.children.push(...items); }
    appendChild(item) { this.children.push(item); }
    setAttribute(name, value) { this[name] = value; }
    set textContent(value) { this.text = value; this.children = []; }
    get textContent() { return this.text; }
    removeAttribute(name) {
      if (name === 'src' && (opts.resetReadyStateOnSrc || opts.chromiumLoadSemantics)) {
        this._src = undefined;
        this.readyState = 0;
        this.currentTime = 0;
        return;
      }
      delete this[name];
    }
    load() {
      if (opts.chromiumLoadSemantics) {
        this._cancelPendingMediaTasks();
      }
    }
    blur() {}
    pause() {
      if (opts.chromiumLoadSemantics) {
        if (this.paused) return;
        this.paused = true;
        if (this._pendingPlays.size) {
          const err = Object.assign(new Error('The play() request was interrupted by a call to pause().'), { name: 'AbortError' });
          for (const reject of [...this._pendingPlays]) {
            this._pendingPlays.delete(reject);
            reject(err);
          }
        }
        const taskId = globalThis.setTimeout(() => {
          this._pendingMediaTasks.delete(taskId);
          this.emit('pause');
        }, 0);
        this._pendingMediaTasks.add(taskId);
        return;
      }
      if (opts.asyncMediaEvents) {
        if (this.paused) return;
        this.paused = true;
        globalThis.setTimeout(() => this.emit('pause'), 0);
        return;
      }
      this.paused = true;
      this.emit('pause');
    }
    play() {
      const err = typeof this.playError === 'function' ? this.playError(this.src) : this.playError;
      if (opts.chromiumLoadSemantics) {
        if (err && err.name !== 'AbortError') return Promise.reject(err);
        const wasPaused = this.paused;
        this.paused = false;
        if (wasPaused) {
          const playTaskId = globalThis.setTimeout(() => {
            this._pendingMediaTasks.delete(playTaskId);
            this.emit('play');
          }, 0);
          this._pendingMediaTasks.add(playTaskId);
        }
        return new Promise((resolve, reject) => {
          const abortReject = (abortErr) => {
            globalThis.clearTimeout(settleTaskId);
            this._pendingMediaTasks.delete(settleTaskId);
            reject(abortErr);
          };
          const settleTaskId = globalThis.setTimeout(() => {
            this._pendingMediaTasks.delete(settleTaskId);
            if (!this._pendingPlays.delete(abortReject)) return;
            if (err) reject(err);
            else resolve();
          }, 0);
          this._pendingPlays.add(abortReject);
          this._pendingMediaTasks.add(settleTaskId);
        });
      }
      if (opts.asyncMediaEvents) {
        if (err && err.name !== 'AbortError') return Promise.reject(err);
        const wasPaused = this.paused;
        this.paused = false;
        if (wasPaused) globalThis.setTimeout(() => this.emit('play'), 0);
        return err ? Promise.reject(err) : Promise.resolve();
      }
      this.paused = false;
      this.emit('play');
      return err ? Promise.reject(err) : Promise.resolve();
    }
    async requestPictureInPicture() {
      documentObj.pictureInPictureElement = this;
      this.emit('enterpictureinpicture');
    }
    canPlayType(mime) { return mime.includes('unsupported') ? '' : 'probably'; }
  }

  const contextGlobals = {
    console,
    document: documentObj,
    window: windowObj,
    navigator: {},
    localStorage: { getItem() { return null; }, setItem() {} },
    setInterval(fn) { context.sync = fn; },
    setTimeout(fn) {
      const id = nextTimerId++;
      timers.set(id, fn);
      return id;
    },
    clearTimeout(id) { timers.delete(id); },
  };
  if (opts.YT) contextGlobals.YT = opts.YT;

  const context = vm.createContext(contextGlobals);
  vm.runInContext(fs.readFileSync(`${__dirname}/src/app.js`, 'utf8') + '\n globalThis.api = {state, audio, video, activeEl, pickVideo, playCurrent, togglePlay, seekTo, onEnded, next, prev, toggleRepeat, toggleShuffle, setQueue, updateModeButtonsUI, goHome, doSearch, openCollection, loadHome, renderResults, renderHome, onTimeUpdate, recoverPlayback};', context);
  async function flushTimers() {
    const entries = [...timers.entries()];
    timers.clear();
    for (const [, fn] of entries) {
      await fn();
    }
    return entries.length;
  }
  return {
    ...context.api,
    sync: context.sync,
    get: (id) => elements.get(id),
    window: windowObj,
    document: documentObj,
    flushTimers,
    pendingTimerCount: () => timers.size,
  };
}

const streams = {
  muxed: [],
  video_only: [{ url: 'video', mime: 'video/mp4', height: 720 }],
  audio: [{ url: 'audio', mime: 'audio/mp4' }],
};
const track = { id: 'one', name: 'One', artist: 'Artist', video: true };

function queued(invoke, opts = {}) {
  const p = player(invoke, undefined, opts);
  p.state.queue = [track];
  p.state.index = 0;
  p.state.wantVideo = true;
  return p;
}

test('split progress and ended use audio master only', () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  p.state.queue.push({ ...track, id: 'two' });
  p.state.mode = 'split';
  assert.equal(p.activeEl(), p.audio);
  p.video.emit('ended');
  assert.equal(p.state.index, 0);
  p.audio.emit('ended');
  assert.equal(p.state.index, 1);
});

test('unsupported MP4 is not preferred over supported video', () => {
  const p = player();
  assert.equal(p.pickVideo([
    { url: 'bad', mime: 'video/mp4; codecs="unsupported"', height: 720 },
    { url: 'good', mime: 'video/webm; codecs="vp9"', height: 720 },
  ], 720).url, 'good');
});

test('failed split startup stops both elements without skipping', async () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  p.video.playError = new Error('unsupported source');
  await p.playCurrent();
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.paused, true);
  assert.equal(p.state.mode, null);
  assert.equal(p.state.index, 0);
  assert.equal(p.get('player').classList.contains('is-loading'), false);
  assert.equal(p.get('pMetaSkeleton').classList.contains('hidden'), true);
});

test('playback skeleton follows stream loading and buffering', async () => {
  let resolveStreams;
  const p = queued(async command => command === 'streams' ? new Promise(resolve => { resolveStreams = resolve; }) : undefined);
  assert.equal(p.get('pMetaSkeleton').classList.contains('hidden'), true);

  const startup = p.playCurrent();
  assert.equal(p.get('player').classList.contains('is-loading'), true);
  assert.equal(p.get('pMetaSkeleton').classList.contains('hidden'), false);
  assert.equal(p.get('playLoading').classList.contains('hidden'), false);

  resolveStreams(streams);
  await startup;
  assert.equal(p.get('player').classList.contains('is-loading'), false);
  assert.equal(p.get('pMetaSkeleton').classList.contains('hidden'), true);

  p.video.readyState = 2;
  p.video.emit('waiting');
  assert.equal(p.get('player').classList.contains('is-loading'), true);
  assert.equal(p.get('pMetaSkeleton').classList.contains('hidden'), false);

  p.video.readyState = 4;
  p.video.emit('canplay');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(p.get('player').classList.contains('is-loading'), false);
  assert.equal(p.get('playLoading').classList.contains('hidden'), true);
});

test('buffering pauses audio and does not repeatedly seek video', async () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  await p.playCurrent();
  p.video.currentTime = 1;
  p.audio.currentTime = 5;
  p.video.readyState = 2;
  p.video.emit('waiting');
  p.sync();
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.currentTime, 1);
  p.video.readyState = 4;
  p.video.emit('canplay');
  await Promise.resolve();
  assert.equal(p.audio.paused, false);
});

test('late stream response cannot replace a newer selection', async () => {
  let resolveOld;
  const p = queued(async (command, args) => {
    if (command !== 'streams') return;
    if (args.videoId === 'one') return new Promise(resolve => { resolveOld = resolve; });
    return { ...streams, video_only: [{ ...streams.video_only[0], url: 'new-video' }] };
  });
  const old = p.playCurrent();
  p.state.queue.push({ ...track, id: 'two' });
  p.state.index = 1;
  await p.playCurrent();
  resolveOld(streams);
  await old;
  assert.equal(p.video.src, 'new-video');
});

test('pause and seek affect both split elements', async () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  await p.playCurrent();
  await p.togglePlay();
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.paused, true);
  p.seekTo(60);
  assert.equal(p.audio.currentTime, 60);
  assert.equal(p.video.currentTime, 60);
});

test('video off preserves paused state and position for a video result', async () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  await p.playCurrent();
  await p.togglePlay();
  p.seekTo(60);
  p.get('videoBtn').emit('click');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(p.state.mode, 'audio');
  assert.equal(p.audio.currentTime, 60);
  assert.equal(p.audio.paused, true);
});

test('intentional play cancellation retains loaded sources', async () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  await p.playCurrent();
  await p.togglePlay();
  p.video.playError = Object.assign(new Error('cancelled play'), { name: 'AbortError' });
  await p.togglePlay();
  assert.equal(p.state.mode, 'split');
  assert.equal(p.video.src, 'video');
});

test('pause while fetching invalidates the pending startup', async () => {
  let resolveStream;
  const p = queued(async command => command === 'streams' ? new Promise(resolve => { resolveStream = resolve; }) : undefined);
  const startup = p.playCurrent();
  assert.equal(p.get('player').classList.contains('is-loading'), true);
  await p.togglePlay();
  assert.equal(p.get('player').classList.contains('is-loading'), false);
  resolveStream(streams);
  await startup;
  assert.equal(p.state.mode, null);
  assert.equal(p.state.wantPlaying, false);
  assert.equal(p.audio.paused, true);
  assert.equal(p.get('pMetaSkeleton').classList.contains('hidden'), true);
});

test('late radio response cannot replace a newer track', async () => {
  let resolveRadio;
  const p = queued(async command => command === 'radio' ? new Promise(resolve => { resolveRadio = resolve; }) : command === 'streams' ? streams : undefined);
  await p.playCurrent();
  const radio = p.next(true);
  p.state.queue.push({ ...track, id: 'two' });
  p.state.index = 1;
  await p.playCurrent();
  resolveRadio([{ ...track, id: 'three' }]);
  await radio;
  assert.equal(p.state.index, 1);
  assert.equal(p.state.queue.length, 2);
});

test('clearing search restores home without changing playback or queue', async () => {
  const p = queued(async command => command === 'search' ? [track] : command === 'streams' ? streams : undefined);
  await p.playCurrent();
  p.get('search').value = 'one';
  await p.doSearch();
  assert.equal(p.state.view, 'search');
  p.get('clearSearch').emit('click');
  assert.equal(p.state.view, 'home');
  assert.equal(p.get('home').classList.contains('hidden'), false);
  assert.equal(p.get('results').classList.contains('hidden'), true);
  assert.equal(p.state.queue[0].id, 'one');
  assert.equal(p.state.mode, 'split');
  assert.equal(p.audio.paused, false);
});

test('late search cannot replace home after clearing input', async () => {
  let resolveSearch;
  const p = player(async command => command === 'search' ? new Promise(resolve => { resolveSearch = resolve; }) : undefined);
  p.get('search').value = 'one';
  const pending = p.doSearch();
  p.get('search').value = '';
  p.get('search').emit('input');
  resolveSearch([track]);
  await pending;
  assert.equal(p.state.view, 'home');
  assert.equal(p.state.results.length, 0);
  assert.equal(p.state.loading, false);
});

test('collection opens tracks without autoplay and supports back home', async () => {
  const p = player(async command => command === 'collection_tracks' ? [track] : undefined);
  await p.openCollection({ id: 'playlist', name: 'My playlist', kind: 'playlist' });
  assert.equal(p.state.view, 'collection');
  assert.equal(p.state.results[0].id, 'one');
  assert.equal(p.state.queue.length, 0);
  assert.equal(p.get('collectionTitle').textContent, 'My playlist');
  p.get('backHome').emit('click');
  assert.equal(p.state.view, 'home');
});

test('late collection cannot replace a newer search', async () => {
  let resolveCollection;
  const p = player(async command => command === 'collection_tracks' ? new Promise(resolve => { resolveCollection = resolve; }) : command === 'search' ? [{ ...track, id: 'new' }] : undefined);
  const pending = p.openCollection({ id: 'playlist', name: 'My playlist', kind: 'playlist' });
  p.get('search').value = 'new';
  await p.doSearch();
  resolveCollection([track]);
  await pending;
  assert.equal(p.state.view, 'search');
  assert.equal(p.state.results[0].id, 'new');
});

test('a failed home section does not hide successful sections and refresh retries', async () => {
  let failing = true;
  const p = player(undefined, async ({ section }) => {
    if (section === 'community' && failing) throw new Error('offline');
    return { items: [{ ...track, kind: 'track' }] };
  });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(p.state.homeLoaded, true);
  assert.equal(p.state.homeData.community.error, 'Error: offline');
  assert.equal(p.state.homeData.releases.items.length, 1);
  assert.equal(p.get('refreshHome').disabled, false);
  failing = false;
  await p.loadHome();
  assert.equal(p.state.homeData.community.items.length, 1);
  assert.equal(p.state.homeData.community.error, undefined);
});

test('toggleRepeat cycles through off, all, one, off with UI updates', () => {
  const p = player();
  assert.equal(p.state.repeat, 'off');
  assert.equal(p.get('repeatBtn').classList.contains('active'), false);

  p.toggleRepeat();
  assert.equal(p.state.repeat, 'all');
  assert.equal(p.get('repeatBtn').classList.contains('active'), true);
  assert.equal(p.get('repeatBadge').classList.contains('hidden'), true);

  p.toggleRepeat();
  assert.equal(p.state.repeat, 'one');
  assert.equal(p.get('repeatBtn').classList.contains('active'), true);
  assert.equal(p.get('repeatBadge').classList.contains('hidden'), false);

  p.toggleRepeat();
  assert.equal(p.state.repeat, 'off');
  assert.equal(p.get('repeatBtn').classList.contains('active'), false);
  assert.equal(p.get('repeatBadge').classList.contains('hidden'), true);
});

test('repeat one replays current track on ended', async () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  p.state.queue.push({ ...track, id: 'two' });
  p.state.repeat = 'one';
  p.state.mode = 'audio';
  p.audio.emit('ended');
  assert.equal(p.state.index, 0);
});

test('manual next advances track even when repeat one is active', async () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  p.state.queue.push({ ...track, id: 'two' });
  p.state.repeat = 'one';
  await p.next(false);
  assert.equal(p.state.index, 1);
});

test('repeat all loops back to start of queue at end', async () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  p.state.queue.push({ ...track, id: 'two' });
  p.state.index = 1;
  p.state.repeat = 'all';
  await p.next(true);
  assert.equal(p.state.index, 0);
});

test('repeat all prev wraps from first track to last track', () => {
  const p = queued();
  p.state.queue.push({ ...track, id: 'two' }, { ...track, id: 'three' });
  p.state.index = 0;
  p.state.repeat = 'all';
  p.activeEl().currentTime = 0;
  p.prev();
  assert.equal(p.state.index, 2);
});

test('shuffle toggle preserves current song and restores original queue when turned off', () => {
  const p = player();
  const tracks = [
    { id: '1', name: 'Song 1' },
    { id: '2', name: 'Song 2' },
    { id: '3', name: 'Song 3' },
    { id: '4', name: 'Song 4' },
  ];
  p.setQueue(tracks, 1); // playing Song 2
  assert.equal(p.state.index, 1);
  assert.equal(p.state.queue[1].id, '2');

  p.toggleShuffle();
  assert.equal(p.state.shuffle, true);
  assert.equal(p.get('shuffleBtn').classList.contains('active'), true);
  // Current track is still Song 2 at index 1
  assert.equal(p.state.queue[p.state.index].id, '2');
  assert.equal(p.state.queue.length, 4);

  // Turn off shuffle
  p.toggleShuffle();
  assert.equal(p.state.shuffle, false);
  assert.equal(p.get('shuffleBtn').classList.contains('active'), false);
  assert.equal(p.state.queue.map(t => t.id).join(','), '1,2,3,4');
  assert.equal(p.state.index, 1);
});

test('setQueue with active shuffle plays clicked track first and shuffles remainder', () => {
  const p = player();
  p.state.shuffle = true;
  const tracks = [
    { id: '1', name: 'Song 1' },
    { id: '2', name: 'Song 2' },
    { id: '3', name: 'Song 3' },
    { id: '4', name: 'Song 4' },
  ];
  p.setQueue(tracks, 2); // picked Song 3
  assert.equal(p.state.index, 0);
  assert.equal(p.state.queue[0].id, '3');
  assert.equal(p.state.queue.length, 4);
  assert.equal(p.state.originalQueue.length, 4);

  // Turn off shuffle
  p.toggleShuffle();
  assert.equal(p.state.queue.map(t => t.id).join(','), '1,2,3,4');
  assert.equal(p.state.index, 2);
});

test('video stage shows native video and hides ytPlayer in video mode', async () => {
  const p = queued(async command => command === 'streams' ? streams : undefined);
  await p.playCurrent();
  assert.equal(p.get('videoStage').classList.contains('hidden'), false);
  assert.equal(p.get('video').classList.contains('hidden'), false);
  assert.equal(p.get('ytPlayer').classList.contains('hidden'), true);
  assert.equal(p.state.mode, 'split');
});

test('pip button toggles and responds to enter/leave picture in picture', () => {
  const p = player();
  const pipBtn = p.get('pipBtn');
  assert.ok(pipBtn);
  assert.equal(pipBtn.classList.contains('active'), false);
  p.video.emit('enterpictureinpicture');
  assert.equal(pipBtn.classList.contains('active'), true);
  p.video.emit('leavepictureinpicture');
  assert.equal(pipBtn.classList.contains('active'), false);
});

// --- F4: Mid-stream stall/error recovery & position preservation ---

test('audio mid-stream error tries remaining candidates at resumePos, refreshes streams up to cap, never uses ytPlayer', async () => {
  let streamCalls = 0;
  let ytLoaded = false;
  class StubYTPlayer {
    constructor() {}
    loadVideoById() { ytLoaded = true; }
    stopVideo() {}
    setVolume() {}
  }
  const p = player(
    async (command) => {
      if (command !== 'streams') return;
      streamCalls++;
      return {
        muxed: [],
        video_only: [],
        audio: [
          { url: `audio-mp4-${streamCalls}`, mime: 'audio/mp4; codecs="mp4a.40.2"' },
          { url: `audio-webm-${streamCalls}`, mime: 'audio/webm; codecs="opus"' },
        ],
      };
    },
    undefined,
    {
      resetReadyStateOnSrc: true,
      YT: { Player: StubYTPlayer, PlayerState: { PLAYING: 1, PAUSED: 2, ENDED: 0, BUFFERING: 3 } },
    }
  );
  p.window.onYouTubeIframeAPIReady();
  p.state.queue = [{ id: 'kalah2', name: 'Kalah 2', artist: 'Aftershine', video: false }];
  p.state.index = 0;
  p.state.wantVideo = false;

  await p.playCurrent();
  assert.equal(streamCalls, 1);
  assert.equal(p.audio.src, 'audio-mp4-1');
  p.audio.readyState = 4;
  p.audio.currentTime = 64.2;

  // 1st mid-stream error: switches to remaining webm candidate & restores 64.2 on loadedmetadata
  p.audio.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 1);
  assert.equal(p.audio.src, 'audio-webm-1');
  assert.equal(p.audio.readyState, 0);
  // Before loadedmetadata, currentTime is not yet applied
  p.audio.readyState = 1;
  p.audio.emit('loadedmetadata');
  assert.equal(p.audio.currentTime, 64.2);
  assert.equal(ytLoaded, false);

  // 2nd mid-stream error: pool exhausted, refreshes streams (refresh #1)
  p.audio.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 2);
  assert.equal(p.audio.src, 'audio-mp4-2');
  p.audio.readyState = 1;
  p.audio.emit('loadedmetadata');
  assert.equal(p.audio.currentTime, 64.2);

  // Exhaust remaining candidate + refresh #2 + failPlayback when cap (2) is exceeded
  p.audio.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(p.audio.src, 'audio-webm-2');
  p.audio.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 3); // 1 initial + 2 refreshes
  assert.equal(p.audio.src, 'audio-mp4-3');

  // Exhaust final refresh pool -> failPlayback cleanly, never ytPlayer
  p.audio.emit('error');
  await new Promise((r) => setImmediate(r));
  p.audio.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 3);
  assert.equal(p.state.mode, null);
  assert.equal(p.state.wantPlaying, false);
  assert.equal(ytLoaded, false);
});

test('muxed mid-stream error preserves currentTime and keeps native video visible', async () => {
  let streamCalls = 0;
  const p = player(
    async (command) => {
      if (command !== 'streams') return;
      streamCalls++;
      return {
        muxed: [
          { url: `muxed-1-${streamCalls}`, mime: 'video/mp4; codecs="avc1.4d401f"', height: 720 },
          { url: `muxed-2-${streamCalls}`, mime: 'video/webm; codecs="vp9"', height: 720 },
        ],
        video_only: [],
        audio: [],
      };
    },
    undefined,
    { resetReadyStateOnSrc: true }
  );
  p.state.queue = [track];
  p.state.index = 0;
  p.state.wantVideo = true;

  await p.playCurrent();
  assert.equal(p.state.mode, 'muxed');
  assert.equal(p.video.src, 'muxed-1-1');
  p.video.readyState = 4;
  p.video.currentTime = 78.5;

  p.video.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(p.state.mode, 'muxed');
  assert.equal(p.video.src, 'muxed-2-1');
  assert.equal(p.get('videoStage').classList.contains('hidden'), false);
  assert.equal(p.get('video').classList.contains('hidden'), false);
  assert.equal(p.get('ytPlayer').classList.contains('hidden'), true);
  p.video.readyState = 1;
  p.video.emit('loadedmetadata');
  assert.equal(p.video.currentTime, 78.5);
});

test('stalled and waiting in audio and muxed modes buffer, clear on playing/timeupdate, and recover on timeout', async () => {
  let streamCalls = 0;
  const p = player(async (command) => {
    if (command !== 'streams') return;
    streamCalls++;
    return {
      muxed: [{ url: `mux-${streamCalls}`, mime: 'video/mp4; codecs="avc1.4d401f"', height: 720 }],
      video_only: [],
      audio: [{ url: `aud-${streamCalls}`, mime: 'audio/mp4' }],
    };
  });
  p.state.queue = [{ id: 'a1', name: 'Song', artist: 'A', video: false }];
  p.state.index = 0;
  p.state.wantVideo = false;
  await p.playCurrent();
  p.audio.currentTime = 64;
  p.audio.readyState = 2;

  // Audio waiting sets Buffering... and clears on playing
  p.audio.emit('waiting');
  assert.equal(p.state.buffering, true);
  assert.equal(p.get('status').classList.contains('hidden'), false);
  assert.ok(p.state.recoveryTimer !== null);
  p.audio.readyState = 4;
  p.audio.emit('playing');
  assert.equal(p.state.buffering, false);
  assert.equal(p.state.recoveryTimer, null);
  assert.equal(p.get('status').classList.contains('hidden'), true);

  // Audio stalled without progress recovers on stall timer at 64s
  p.audio.readyState = 2;
  p.audio.emit('stalled');
  assert.equal(p.state.buffering, true);
  assert.ok(p.state.recoveryTimer !== null);
  await p.flushTimers();
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 2);
  assert.equal(p.audio.src, 'aud-2');
  assert.equal(p.audio.currentTime, 64);

  // Muxed stalled clears via progressing timeupdate
  p.state.wantVideo = true;
  await p.playCurrent();
  assert.equal(p.state.mode, 'muxed');
  p.video.currentTime = 30;
  p.video.readyState = 2;
  p.video.emit('stalled');
  assert.equal(p.state.buffering, true);
  p.video.readyState = 4;
  p.video.currentTime = 31;
  p.video.emit('timeupdate');
  assert.equal(p.state.buffering, false);
  assert.equal(p.state.recoveryTimer, null);
});

test('split waiting preserves pause icon and recovers via stall timer if canplay never fires; ended does not stall-recover', async () => {
  let streamCalls = 0;
  const p = queued(async (command) => {
    if (command !== 'streams') return;
    streamCalls++;
    return {
      muxed: [],
      video_only: [{ url: `v-${streamCalls}`, mime: 'video/mp4', height: 720 }],
      audio: [{ url: `a-${streamCalls}`, mime: 'audio/mp4' }],
    };
  });
  await p.playCurrent();
  p.audio.currentTime = 64.5;
  p.video.readyState = 2;

  p.video.emit('waiting');
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.paused, true);
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.state.splitBufferingPaused, true);
  // Pause icon stays visible during buffering (play icon stays hidden)
  assert.equal(p.get('iconPlay').classList.contains('hidden'), true);
  assert.equal(p.get('iconPause').classList.contains('hidden'), false);

  // canplay never arrives -> stall timer triggers recovery
  await p.flushTimers();
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 2);
  assert.equal(p.audio.src, 'a-2');
  assert.equal(p.audio.currentTime, 64.5);

  // Ended clears recovery timer and does not trigger stall recovery
  p.audio.ended = true;
  p.audio.emit('stalled');
  assert.equal(p.state.recoveryTimer, null);
  p.audio.emit('ended');
  assert.equal(p.state.recoveryTimer, null);
});

test('rapid track switch cancels in-flight recovery via playbackId guard', async () => {
  let resolveRefresh;
  let streamCalls = 0;
  const p = queued(async (command, args) => {
    if (command !== 'streams') return;
    streamCalls++;
    if (args.videoId === 'one' && streamCalls === 2) {
      return new Promise((resolve) => { resolveRefresh = resolve; });
    }
    return {
      muxed: [],
      video_only: [{ url: `v-${args.videoId}`, mime: 'video/mp4', height: 720 }],
      audio: [{ url: `a-${args.videoId}`, mime: 'audio/mp4' }],
    };
  });
  p.state.queue.push({ ...track, id: 'two', name: 'Two' });
  await p.playCurrent();
  p.audio.currentTime = 64;

  // Trigger recovery for track one
  p.audio.emit('error');
  await Promise.resolve();
  assert.equal(p.state.recovering, true);

  // User immediately switches to track two
  p.state.index = 1;
  await p.playCurrent();
  assert.equal(p.audio.src, 'a-two');
  assert.equal(p.state.recovering, false);

  // Old refresh resolves late -> must NOT overwrite track two
  resolveRefresh({
    muxed: [],
    video_only: [{ url: 'stale-v', mime: 'video/mp4', height: 720 }],
    audio: [{ url: 'stale-a', mime: 'audio/mp4' }],
  });
  await new Promise((r) => setImmediate(r));
  assert.equal(p.audio.src, 'a-two');
  assert.equal(p.video.src, 'v-two');
});

test('error while starting in paused state (wantPlaying false) recovers without auto-playing', async () => {
  let streamCalls = 0;
  const p = player(
    async (command) => {
      if (command !== 'streams') return;
      streamCalls++;
      return {
        muxed: [],
        video_only: [],
        audio: [{ url: `aud-${streamCalls}`, mime: 'audio/mp4' }],
      };
    },
    undefined,
    { resetReadyStateOnSrc: true }
  );
  p.state.queue = [{ id: 'a1', name: 'Song', artist: 'A', video: false }];
  p.state.index = 0;
  p.state.wantVideo = false;
  await p.playCurrent(0, false);
  assert.equal(streamCalls, 1);
  assert.equal(p.audio.src, 'aud-1');
  p.state.starting = true;
  p.audio.readyState = 4;
  p.audio.currentTime = 42;
  p.audio.error = { code: 2 };

  p.audio.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 2);
  assert.equal(p.audio.src, 'aud-2');
  p.audio.readyState = 1;
  p.audio.emit('loadedmetadata');
  assert.equal(p.audio.currentTime, 42);
  assert.equal(p.audio.paused, true);
  assert.equal(p.state.wantPlaying, false);
});

// --- F5: Reliable seek restoration, transitions & concurrency guards ---

test('applyInitialPosition defers seek until loadedmetadata when readyState is 0 and ignores stale events', async () => {
  const p = player(
    async (command, args) => command === 'streams'
      ? {
          muxed: [],
          video_only: [{ url: `v-${args.videoId}`, mime: 'video/mp4', height: 720 }],
          audio: [{ url: `a-${args.videoId}`, mime: 'audio/mp4' }],
        }
      : undefined,
    undefined,
    { resetReadyStateOnSrc: true }
  );
  p.state.queue = [track, { ...track, id: 'two', name: 'Two' }];
  p.state.index = 0;
  p.state.wantVideo = true;

  await p.playCurrent();
  p.audio.readyState = 1;
  p.audio.emit('loadedmetadata');
  p.video.readyState = 1;
  p.video.emit('loadedmetadata');
  p.audio.readyState = 4;
  p.video.readyState = 4;

  p.seekTo(60);
  await p.togglePlay();
  p.get('videoBtn').emit('click');
  await new Promise((r) => setImmediate(r));

  // Audio src changed -> readyState reset to 0 -> currentTime must wait for loadedmetadata
  assert.equal(p.state.mode, 'audio');
  assert.equal(p.audio.readyState, 0);
  assert.equal(p.audio.currentTime, 0);

  p.audio.readyState = 1;
  p.audio.emit('loadedmetadata');
  assert.equal(p.audio.currentTime, 60);

  // Stale loadedmetadata from track 1 after switching to track 2 must be ignored
  const p2 = player(
    async (command, args) => command === 'streams'
      ? { muxed: [], video_only: [], audio: [{ url: `a-${args.videoId}`, mime: 'audio/mp4' }] }
      : undefined,
    undefined,
    { resetReadyStateOnSrc: true }
  );
  p2.state.queue = [{ id: 't1', name: '1', video: false }, { id: 't2', name: '2', video: false }];
  p2.state.index = 0;
  await p2.playCurrent(60, true);
  p2.state.index = 1;
  await p2.playCurrent(0, true);
  p2.audio.readyState = 1;
  p2.audio.emit('loadedmetadata');
  assert.equal(p2.audio.currentTime, 0);
});

test('next() radio concurrency lock prevents duplicate radio calls and unlocks on error', async () => {
  let resolveRadio;
  let radioCalls = 0;
  let shouldReject = false;
  const p = queued(async (command) => {
    if (command === 'streams') return streams;
    if (command === 'radio') {
      radioCalls++;
      if (shouldReject) throw new Error('radio down');
      return new Promise((resolve) => { resolveRadio = resolve; });
    }
  });
  await p.playCurrent();

  const r1 = p.next(true);
  const r2 = p.next(false);
  assert.equal(radioCalls, 1);
  resolveRadio([{ ...track, id: 'two' }]);
  await Promise.all([r1, r2]);
  assert.equal(p.state.queue.length, 2);
  assert.equal(p.state.index, 1);

  // Error unlocks radioPendingId so subsequent next() retries cleanly
  shouldReject = true;
  await p.next(true);
  assert.equal(radioCalls, 2);
  shouldReject = false;
  const r3 = p.next(false);
  assert.equal(radioCalls, 3);
  resolveRadio([{ ...track, id: 'three' }]);
  await r3;
  assert.equal(p.state.queue.length, 3);
  assert.equal(p.state.index, 2);
});

test('ytPlayer callbacks are ignored when state.mode is not yt', () => {
  let capturedEvents = null;
  class StubYTPlayer {
    constructor(_, config) { capturedEvents = config.events; }
    setVolume() {}
    stopVideo() {}
  }
  const p = queued(
    async (command) => command === 'streams' ? streams : undefined,
    { YT: { Player: StubYTPlayer, PlayerState: { PLAYING: 1, PAUSED: 2, ENDED: 0, BUFFERING: 3 } } }
  );
  p.window.onYouTubeIframeAPIReady();
  p.state.queue.push({ ...track, id: 'two' });
  p.state.mode = 'audio';
  p.state.wantPlaying = true;
  p.state.index = 0;

  capturedEvents.onStateChange({ data: 2 }); // PAUSED
  assert.equal(p.state.wantPlaying, true);
  capturedEvents.onStateChange({ data: 0 }); // ENDED
  assert.equal(p.state.index, 0);
  assert.equal(p.state.wantPlaying, true);
  capturedEvents.onError({ data: 150 });
  assert.equal(p.state.index, 0);
});

test('state.seeking resets on cancelled drag (pointercancel, blur, window blur, seeked)', async () => {
  const p = queued(async (command) => command === 'streams' ? streams : undefined);
  await p.playCurrent();
  const progress = p.get('progress');

  progress.emit('input');
  assert.equal(p.state.seeking, true);
  progress.emit('pointercancel');
  assert.equal(p.state.seeking, false);

  progress.emit('input');
  assert.equal(p.state.seeking, true);
  progress.emit('blur');
  assert.equal(p.state.seeking, false);

  progress.emit('input');
  assert.equal(p.state.seeking, true);
  p.window.emit('blur');
  assert.equal(p.state.seeking, false);

  progress.emit('input');
  assert.equal(p.state.seeking, true);
  p.audio.emit('seeked');
  assert.equal(p.state.seeking, false);

  p.audio.currentTime = 65;
  p.audio.emit('timeupdate');
  assert.equal(p.get('curTime').textContent, '1:05');
});

test('search result click clears wantVideo when selecting audio track after video and exits PiP', async () => {
  const p = queued(async (command) => command === 'streams' ? streams : undefined);
  await p.playCurrent();
  await p.video.requestPictureInPicture();
  assert.equal(p.document.pictureInPictureElement, p.video);
  assert.equal(p.state.wantVideo, true);

  p.state.view = 'search';
  p.state.results = [{ id: 'a1', name: 'Audio Only', artist: 'Art', video: false }];
  p.renderResults();
  const li = p.get('results').children[0];
  li.click();
  await new Promise((r) => setImmediate(r));

  assert.equal(p.state.wantVideo, false);
  assert.equal(p.state.mode, 'audio');
  assert.equal(p.document.pictureInPictureElement, null);
});

test('muxed playback uses video element, seeks video, and advances only on video ended', async () => {
  const muxedStreams = {
    muxed: [{ url: 'muxed-url', mime: 'video/mp4; codecs="avc1.4d401f"', height: 720 }],
    video_only: [],
    audio: [],
  };
  const p = queued(async (command) => command === 'streams' ? muxedStreams : undefined);
  p.state.queue.push({ ...track, id: 'two' });
  await p.playCurrent();

  assert.equal(p.state.mode, 'muxed');
  assert.equal(p.activeEl(), p.video);
  assert.equal(p.video.src, 'muxed-url');
  assert.equal(p.video.muted, false);
  assert.equal(p.video.paused, false);
  assert.equal(p.audio.paused, true);

  p.seekTo(45);
  assert.equal(p.video.currentTime, 45);
  p.audio.emit('ended');
  assert.equal(p.state.index, 0);
  p.video.emit('ended');
  assert.equal(p.state.index, 1);
});

// --- F6 / R2: PiP controls, split/muxed PiP sync, 16:9 framing & CSS token completeness ---

test('pipBtn click toggles PiP on/off and videoBtn off exits PiP', async () => {
  const p = queued(async (command) => command === 'streams' ? streams : undefined);
  await p.playCurrent();
  const pipBtn = p.get('pipBtn');

  pipBtn.emit('click');
  await Promise.resolve();
  assert.equal(p.document.pictureInPictureElement, p.video);
  assert.equal(pipBtn.classList.contains('active'), true);

  pipBtn.emit('click');
  await Promise.resolve();
  assert.equal(p.document.pictureInPictureElement, null);
  assert.equal(pipBtn.classList.contains('active'), false);

  pipBtn.emit('click');
  await Promise.resolve();
  assert.equal(p.document.pictureInPictureElement, p.video);
  p.get('videoBtn').emit('click');
  await new Promise((r) => setImmediate(r));
  assert.equal(p.document.pictureInPictureElement, null);
  assert.equal(pipBtn.classList.contains('active'), false);
});

test('split-mode external PiP pause and play synchronize audio, video, wantPlaying, and UI', async () => {
  const p = queued(async (command) => command === 'streams' ? streams : undefined);
  await p.playCurrent();
  assert.equal(p.state.mode, 'split');
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);

  // External PiP pauses <video> directly
  p.video.pause();
  assert.equal(p.audio.paused, true);
  assert.equal(p.state.wantPlaying, false);
  assert.equal(p.get('iconPlay').classList.contains('hidden'), false);
  assert.equal(p.get('iconPause').classList.contains('hidden'), true);

  // Single togglePlay() resumes cleanly
  await p.togglePlay();
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
  assert.equal(p.state.wantPlaying, true);

  // Pause again, then external PiP plays <video> directly
  p.video.pause();
  assert.equal(p.audio.paused, true);
  await p.video.play();
  await Promise.resolve();
  assert.equal(p.audio.paused, false);
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.get('iconPlay').classList.contains('hidden'), true);
  assert.equal(p.get('iconPause').classList.contains('hidden'), false);
});

test('muxed-mode external PiP pause and play update state.wantPlaying without double-click desync', async () => {
  const muxedStreams = {
    muxed: [{ url: 'muxed-url', mime: 'video/mp4; codecs="avc1.4d401f"', height: 720 }],
    video_only: [],
    audio: [],
  };
  const p = queued(async (command) => command === 'streams' ? muxedStreams : undefined);
  await p.playCurrent();
  assert.equal(p.state.mode, 'muxed');

  p.video.pause();
  assert.equal(p.state.wantPlaying, false);
  await p.togglePlay(); // one click resumes
  assert.equal(p.video.paused, false);
  assert.equal(p.state.wantPlaying, true);

  await p.togglePlay(); // pause
  await p.video.play(); // external PiP play
  assert.equal(p.state.wantPlaying, true);
  await p.togglePlay(); // one click pauses
  assert.equal(p.video.paused, true);
  assert.equal(p.state.wantPlaying, false);
});

test('R2 / F6 static invariants: 16:9 uncropped video, right-click PiP enabled, 0 undefined CSS vars', () => {
  const html = fs.readFileSync(`${__dirname}/src/index.html`, 'utf8');
  const css = fs.readFileSync(`${__dirname}/src/styles.css`, 'utf8');
  const appJs = fs.readFileSync(`${__dirname}/src/app.js`, 'utf8');

  // HTML & JS PiP / contextmenu checks
  assert.match(html, /<video[^>]*id="video"[^>]*playsinline/i);
  assert.doesNotMatch(html, /disablepictureinpicture/i);
  assert.doesNotMatch(appJs, /contextmenu/i);

  // 16:9 uncropped CSS checks
  assert.match(css, /\.stage\s*\{[^}]*aspect-ratio:\s*16\s*\/\s*9/s);
  assert.match(css, /\.stage\s+video\s*\{[^}]*aspect-ratio:\s*16\s*\/\s*9/s);
  assert.match(css, /\.stage\s+video\s*\{[^}]*object-fit:\s*contain/s);

  // CSS custom property completeness: every var(--*) must exist in :root
  const lines = css.split(/\r?\n/);
  const rs = lines.findIndex((l) => l.trim().startsWith(':root'));
  const re = lines.findIndex((l, i) => i > rs && l.trim() === '}');
  assert.ok(rs !== -1 && re > rs);
  const defined = new Set();
  for (let i = rs; i <= re; i++) {
    const m = lines[i].match(/^\s*(--[a-zA-Z0-9_-]+)\s*:/);
    if (m) defined.add(m[1]);
  }
  const missing = [];
  lines.forEach((l, i) => {
    for (const m of l.matchAll(/var\(\s*(--[a-zA-Z0-9_-]+)/g)) {
      if (!defined.has(m[1])) missing.push(`${i + 1}:${m[1]}`);
    }
  });
  assert.deepEqual(missing, []);
});

// --- Iteration 2 Regression Tests ---

test('split-mode async queued play/pause events do not oscillate across waiting->canplay, rapid togglePlay, recoverPlayback, or PiP', async () => {
  let streamCalls = 0;
  const p = queued(
    async (command) => {
      if (command !== 'streams') return;
      streamCalls++;
      return {
        muxed: [],
        video_only: [{ url: `v-${streamCalls}`, mime: 'video/mp4', height: 720 }],
        audio: [{ url: `a-${streamCalls}`, mime: 'audio/mp4' }],
      };
    },
    { asyncMediaEvents: true, resetReadyStateOnSrc: true }
  );
  let audioPlayEvents = 0;
  let audioPauseEvents = 0;
  let videoPlayEvents = 0;
  let videoPauseEvents = 0;
  p.audio.addEventListener('play', () => { audioPlayEvents++; });
  p.audio.addEventListener('pause', () => { audioPauseEvents++; });
  p.video.addEventListener('play', () => { videoPlayEvents++; });
  p.video.addEventListener('pause', () => { videoPauseEvents++; });

  await p.playCurrent();
  await new Promise((r) => setTimeout(r, 15));
  p.audio.readyState = 4;
  p.video.readyState = 4;
  assert.equal(p.state.mode, 'split');
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);

  // Scenario A: waiting -> canplay before queued pause macrotasks fire
  p.video.readyState = 2;
  p.video.emit('waiting');
  assert.equal(p.state.splitBufferingPaused, true);
  p.video.readyState = 4;
  p.video.emit('canplay');
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.state.splitBufferingPaused, false);
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
  assert.equal(audioPauseEvents, 1);
  assert.equal(videoPauseEvents, 1);

  // Scenario B: rapid togglePlay() pause -> play before queued pause macrotasks fire
  const beforeBPlay = audioPlayEvents;
  const beforeBPause = audioPauseEvents;
  await p.togglePlay(); // pause
  await p.togglePlay(); // immediately play
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
  assert.equal(audioPauseEvents - beforeBPause, 1);
  assert.equal(audioPlayEvents - beforeBPlay, 1);

  // Scenario C: recoverPlayback in split mode clears recovering before macrotasks fire
  const beforeCRecoverPlay = audioPlayEvents;
  const beforeCRecoverPause = audioPauseEvents;
  p.audio.currentTime = 40;
  p.audio.error = { code: 2 };
  p.audio.emit('error');
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(streamCalls, 2);
  assert.equal(p.state.recovering, false);
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
  assert.equal(audioPauseEvents - beforeCRecoverPause, 1);
  assert.equal(audioPlayEvents - beforeCRecoverPlay, 1);

  // Real external PiP pause and play still synchronize cleanly under async events
  p.video.pause();
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(p.state.wantPlaying, false);
  assert.equal(p.audio.paused, true);
  assert.equal(p.get('iconPlay').classList.contains('hidden'), false);

  await p.video.play();
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.audio.paused, false);
  assert.equal(p.get('iconPause').classList.contains('hidden'), false);
});

test('split-mode video stalled is not cancelled by playing audio timeupdate or playing', async () => {
  let streamCalls = 0;
  const p = queued(async (command) => {
    if (command !== 'streams') return;
    streamCalls++;
    return {
      muxed: [],
      video_only: [{ url: `v-${streamCalls}`, mime: 'video/mp4', height: 720 }],
      audio: [{ url: `a-${streamCalls}`, mime: 'audio/mp4' }],
    };
  });
  await p.playCurrent();
  p.audio.readyState = 4;
  p.audio.currentTime = 50;
  p.video.readyState = 2;

  p.video.emit('stalled');
  assert.equal(p.state.buffering, true);
  assert.equal(p.get('status').classList.contains('hidden'), false);
  assert.ok(p.state.recoveryTimer !== null);

  // Audio continues advancing and emitting timeupdate/playing while video is still stalled (< 3)
  p.audio.currentTime = 51;
  p.audio.emit('timeupdate');
  p.audio.emit('playing');
  assert.equal(p.state.buffering, true);
  assert.equal(p.get('status').classList.contains('hidden'), false);
  assert.ok(p.state.recoveryTimer !== null);

  // Stall timer fires and recovers since video never became ready
  await p.flushTimers();
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 2);
  assert.equal(p.video.src, 'v-2');
  assert.equal(p.audio.src, 'a-2');

  // Once both elements have readyState >= 3, timeupdate clears buffering & recoveryTimer
  p.video.readyState = 2;
  p.video.emit('stalled');
  assert.equal(p.state.buffering, true);
  p.video.readyState = 4;
  p.audio.readyState = 4;
  p.audio.currentTime = 53;
  p.audio.emit('timeupdate');
  assert.equal(p.state.buffering, false);
  assert.equal(p.state.recoveryTimer, null);
  assert.equal(p.get('status').classList.contains('hidden'), true);
});

test('split refresh tries (v1, a0) when v0 rejects without error and a1 fails; wantVideo falls back to audio if muxed+split fail', async () => {
  // Part 1: (v1-good, a0-good) succeeds when v0-bad rejects play() without video.error and a1-bad rejects play()
  let streamCalls = 0;
  const p = queued(
    async (command) => {
      if (command !== 'streams') return;
      streamCalls++;
      if (streamCalls === 1) {
        return {
          muxed: [],
          video_only: [{ url: 'v-init', mime: 'video/mp4', height: 720 }],
          audio: [{ url: 'a-init', mime: 'audio/mp4' }],
        };
      }
      return {
        muxed: [],
        video_only: [
          { url: 'v0-bad', mime: 'video/mp4', height: 720 },
          { url: 'v1-good', mime: 'video/mp4', height: 720 },
        ],
        audio: [
          { url: 'a0-good', mime: 'audio/mp4' },
          { url: 'a1-bad', mime: 'audio/mp4' },
        ],
      };
    },
    { resetReadyStateOnSrc: true }
  );
  p.video.playError = (src) => (src === 'v0-bad' ? new Error('bad video candidate') : null);
  p.audio.playError = (src) => (src === 'a1-bad' ? new Error('bad audio candidate') : null);

  await p.playCurrent();
  p.audio.readyState = 4;
  p.video.readyState = 4;
  p.audio.currentTime = 64.2;

  p.audio.error = { code: 2 };
  p.audio.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 2);
  assert.equal(p.state.mode, 'split');
  assert.equal(p.video.src, 'v1-good');
  assert.equal(p.audio.src, 'a0-good');
  p.video.readyState = 1;
  p.video.emit('loadedmetadata');
  p.audio.readyState = 1;
  p.audio.emit('loadedmetadata');
  assert.equal(p.audio.currentTime, 64.2);

  // Part 2: wantVideo true falls back to audio mode when refreshed muxed fails and video_only is empty
  let calls2 = 0;
  const p2 = queued(
    async (command) => {
      if (command !== 'streams') return;
      calls2++;
      if (calls2 === 1) {
        return {
          muxed: [{ url: 'm-init', mime: 'video/mp4', height: 720 }],
          video_only: [],
          audio: [],
        };
      }
      return {
        muxed: [{ url: 'm-bad', mime: 'video/mp4', height: 720 }],
        video_only: [],
        audio: [{ url: 'a-fallback', mime: 'audio/mp4' }],
      };
    },
    { resetReadyStateOnSrc: true }
  );
  p2.video.playError = (src) => (src === 'm-bad' ? new Error('muxed failed') : null);
  await p2.playCurrent();
  p2.video.readyState = 4;
  p2.video.currentTime = 64.2;

  p2.video.error = { code: 2 };
  p2.video.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(calls2, 2);
  assert.equal(p2.state.mode, 'audio');
  assert.equal(p2.audio.src, 'a-fallback');
  p2.audio.readyState = 1;
  p2.audio.emit('loadedmetadata');
  assert.equal(p2.audio.currentTime, 64.2);
});

test('seekTo during splitBufferingPaused resumes ready split streams & clears Buffering; seekTo during in-flight recoverPlayback preserves target', async () => {
  let resolveRefresh;
  let streamCalls = 0;
  const p = queued(
    async (command) => {
      if (command !== 'streams') return;
      streamCalls++;
      if (streamCalls === 2) {
        return new Promise((resolve) => { resolveRefresh = resolve; });
      }
      return {
        muxed: [],
        video_only: [{ url: `v-${streamCalls}`, mime: 'video/mp4', height: 720 }],
        audio: [{ url: `a-${streamCalls}`, mime: 'audio/mp4' }],
      };
    },
    { resetReadyStateOnSrc: true }
  );
  await p.playCurrent();
  p.audio.readyState = 4;
  p.video.readyState = 4;
  p.audio.currentTime = 64.2;

  // Seek while splitBufferingPaused and both elements have readyState >= 3 -> resumes and clears status
  p.video.readyState = 2;
  p.video.emit('waiting');
  assert.equal(p.state.splitBufferingPaused, true);
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.paused, true);
  assert.equal(p.get('status').classList.contains('hidden'), false);

  p.video.readyState = 4;
  p.seekTo(95);
  await Promise.resolve();
  assert.equal(p.state.buffering, false);
  assert.equal(p.state.splitBufferingPaused, false);
  assert.equal(p.get('status').classList.contains('hidden'), true);
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
  assert.equal(p.audio.currentTime, 95);
  assert.equal(p.video.currentTime, 95);

  // Seek while splitBufferingPaused when video.readyState < 3 stays safely buffered
  p.video.readyState = 2;
  p.video.emit('waiting');
  p.seekTo(30);
  assert.equal(p.state.buffering, true);
  assert.equal(p.state.splitBufferingPaused, true);
  assert.equal(p.audio.paused, true);

  // Seek to 120s while recoverPlayback is awaiting invoke("streams") -> preserves 120s instead of stale 30s
  p.audio.error = { code: 2 };
  p.audio.emit('error');
  await Promise.resolve();
  assert.equal(p.state.recovering, true);
  p.seekTo(120);
  resolveRefresh({
    muxed: [],
    video_only: [{ url: 'v-refreshed', mime: 'video/mp4', height: 720 }],
    audio: [{ url: 'a-refreshed', mime: 'audio/mp4' }],
  });
  await new Promise((r) => setImmediate(r));
  p.audio.readyState = 1;
  p.audio.emit('loadedmetadata');
  p.video.readyState = 1;
  p.video.emit('loadedmetadata');
  assert.equal(p.audio.currentTime, 120);
  assert.equal(p.video.currentTime, 120);
});

test('state.loading status is not clobbered by playback status updates and yt fallback exits PiP', async () => {
  let resolveSearch;
  class StubYTPlayer {
    constructor(_, config) {
      Promise.resolve().then(() => config?.events?.onReady?.());
    }
    loadVideoById() {}
    pauseVideo() {}
    stopVideo() {}
    setVolume() {}
  }
  let failStreams = false;
  const p = queued(
    async (command) => {
      if (command === 'search') return new Promise((resolve) => { resolveSearch = resolve; });
      if (command === 'streams') {
        if (failStreams) throw new Error('no streams');
        return streams;
      }
    },
    { YT: { Player: StubYTPlayer, PlayerState: { PLAYING: 1, PAUSED: 2, ENDED: 0, BUFFERING: 3 } } }
  );
  p.window.onYouTubeIframeAPIReady();
  await Promise.resolve();

  await p.playCurrent();
  await p.video.requestPictureInPicture();
  assert.equal(p.document.pictureInPictureElement, p.video);

  // Start search -> status shows "Nyari…" while state.loading is true
  p.get('search').value = 'kalah';
  const searchPromise = p.doSearch();
  assert.equal(p.state.loading, true);
  assert.ok(p.get('status').children.includes('Nyari…'));

  // Playback buffering/recovery/yt fallback must NOT clobber "Nyari…"
  p.audio.readyState = 2;
  p.audio.emit('waiting');
  assert.ok(p.get('status').children.includes('Nyari…'));

  failStreams = true;
  await p.playCurrent();
  assert.equal(p.state.mode, 'yt');
  assert.equal(p.document.pictureInPictureElement, null);
  assert.ok(p.get('status').children.includes('Nyari…'));

  resolveSearch([]);
  await searchPromise;
  assert.equal(p.state.loading, false);
});

// --- Iteration 3 Regression Tests (Chromium load/src cancellation, AbortError, progress change, hygiene) ---

test('Chromium load() semantics: Track 1 -> Track 2 via stopAll() does not leak mediaOps.pause and external PiP pause/play sync cleanly', async () => {
  const p = queued(
    async (command, args) => command === 'streams'
      ? {
          muxed: [],
          video_only: [{ url: `v-${args.videoId}`, mime: 'video/mp4', height: 720 }],
          audio: [{ url: `a-${args.videoId}`, mime: 'audio/mp4' }],
        }
      : undefined,
    { chromiumLoadSemantics: true }
  );
  p.state.queue = [
    { ...track, id: 't1', name: 'Track 1' },
    { ...track, id: 't2', name: 'Track 2' },
  ];
  p.state.index = 0;

  await p.playCurrent();
  await new Promise((r) => setTimeout(r, 15));
  p.audio.readyState = 4;
  p.video.readyState = 4;
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
  assert.equal(p.state.wantPlaying, true);

  // Switch to Track 2 while playing -> stopAll() calls programmaticPause + clearMediaSrc (load() cancels queued pause)
  p.state.index = 1;
  await p.playCurrent();
  await new Promise((r) => setTimeout(r, 15));
  p.audio.readyState = 4;
  p.video.readyState = 4;
  assert.equal(p.audio.src, 'a-t2');
  assert.equal(p.video.src, 'v-t2');
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);

  // External PiP pause on Track 2 must NOT be swallowed by leaked ops.pause
  p.video.pause();
  await new Promise((r) => setTimeout(r, 15));
  assert.equal(p.state.wantPlaying, false);
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.paused, true);
  assert.equal(p.get('iconPlay').classList.contains('hidden'), false);
  assert.equal(p.get('iconPause').classList.contains('hidden'), true);

  // External PiP play on Track 2 must synchronize cleanly
  await p.video.play();
  await new Promise((r) => setTimeout(r, 15));
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
  assert.equal(p.get('iconPlay').classList.contains('hidden'), true);
  assert.equal(p.get('iconPause').classList.contains('hidden'), false);
});

test('Chromium src= semantics: recoverPlayback() while playing does not leak mediaOps.pause and audio fallback clears video.src', async () => {
  let streamCalls = 0;
  const p = queued(
    async (command) => {
      if (command !== 'streams') return;
      streamCalls++;
      return {
        muxed: [],
        video_only: [{ url: `v-${streamCalls}`, mime: 'video/mp4', height: 720 }],
        audio: [{ url: `a-${streamCalls}`, mime: 'audio/mp4' }],
      };
    },
    { chromiumLoadSemantics: true }
  );
  await p.playCurrent();
  await new Promise((r) => setTimeout(r, 15));
  p.audio.readyState = 4;
  p.video.readyState = 4;
  p.audio.currentTime = 64.2;

  // Trigger recovery while playing -> programmaticPause + src= cancels queued pause tasks
  p.audio.error = { code: 2 };
  p.audio.emit('error');
  await new Promise((r) => setTimeout(r, 25));
  assert.equal(streamCalls, 2);
  assert.equal(p.state.recovering, false);
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);

  // External PiP pause after recovery must pause audio & update wantPlaying
  p.video.pause();
  await new Promise((r) => setTimeout(r, 15));
  assert.equal(p.state.wantPlaying, false);
  assert.equal(p.audio.paused, true);
  assert.equal(p.get('iconPlay').classList.contains('hidden'), false);

  // Part 2: recoverPlayback audio fallback clears dead video.src via clearMediaSrc
  let calls2 = 0;
  const p2 = queued(
    async (command) => {
      if (command !== 'streams') return;
      calls2++;
      if (calls2 === 1) {
        return { muxed: [{ url: 'm-dead', mime: 'video/mp4', height: 720 }], video_only: [], audio: [] };
      }
      return { muxed: [], video_only: [], audio: [{ url: 'a-fallback', mime: 'audio/mp4' }] };
    },
    { chromiumLoadSemantics: true }
  );
  await p2.playCurrent();
  await new Promise((r) => setTimeout(r, 15));
  assert.equal(p2.video.src, 'm-dead');
  p2.video.error = { code: 2 };
  p2.video.emit('error');
  await new Promise((r) => setTimeout(r, 25));
  assert.equal(p2.state.mode, 'audio');
  assert.equal(p2.audio.src, 'a-fallback');
  assert.equal(p2.video.src, undefined);
});

test('Chromium AbortError semantics: aborted in-flight play() via stopAll() and pause()-after-play() do not leak mediaOps.play', async () => {
  // Part A: in-flight play() interrupted by stopAll() (load() cancels play task & rejects with AbortError)
  const p = queued(
    async (command, args) => command === 'streams'
      ? {
          muxed: [],
          video_only: [{ url: `v-${args.videoId}`, mime: 'video/mp4', height: 720 }],
          audio: [{ url: `a-${args.videoId}`, mime: 'audio/mp4' }],
        }
      : undefined,
    { chromiumLoadSemantics: true }
  );
  p.state.queue = [
    { ...track, id: 't1', name: 'Track 1' },
    { ...track, id: 't2', name: 'Track 2' },
  ];
  p.state.index = 0;

  const p1 = p.playCurrent();
  // Yield microtask so invoke("streams") resolves and play() is invoked & still in-flight before setTimeout(0)
  await Promise.resolve();
  await Promise.resolve();
  p.state.index = 1;
  const p2 = p.playCurrent(); // calls stopAll() while t1 play() is in-flight!
  await Promise.all([p1, p2]);
  await new Promise((r) => setTimeout(r, 20));
  p.audio.readyState = 4;
  p.video.readyState = 4;

  // Pause Track 2, then simulate external PiP video.play() -> must NOT be swallowed by leaked ops.play
  await p.togglePlay();
  await new Promise((r) => setTimeout(r, 15));
  assert.equal(p.state.wantPlaying, false);
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.paused, true);

  await p.video.play();
  await new Promise((r) => setTimeout(r, 15));
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
  assert.equal(p.get('iconPause').classList.contains('hidden'), false);

  // Part B: Blink non-cancelling pause()-after-play() AbortError (same loadSeq) delivers queued play & pause cleanly
  await p.togglePlay(); // pause
  await new Promise((r) => setTimeout(r, 15));
  const resumeAttempt = p.togglePlay(); // calls programmaticPlay (in-flight)
  const pauseAttempt = p.togglePlay();  // calls programmaticPause synchronously -> rejects play() with AbortError
  await Promise.all([resumeAttempt, pauseAttempt]);
  await new Promise((r) => setTimeout(r, 15));
  assert.equal(p.state.wantPlaying, false);
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.paused, true);

  // External PiP video.play() after non-load AbortError must still work
  await p.video.play();
  await new Promise((r) => setTimeout(r, 15));
  assert.equal(p.state.wantPlaying, true);
  assert.equal(p.audio.paused, false);
});

test('Chromium waiting -> canplay interrupted mid-flight by new waiting (AbortError) preserves splitBufferingPaused', async () => {
  const p = queued(async (command) => command === 'streams' ? streams : undefined, { chromiumLoadSemantics: true });
  await p.playCurrent();
  await new Promise((r) => setTimeout(r, 15));
  p.audio.readyState = 4;
  p.video.readyState = 4;

  // 1st waiting pauses split pair
  p.video.readyState = 2;
  p.video.emit('waiting');
  assert.equal(p.state.splitBufferingPaused, true);
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.paused, true);

  // canplay starts async Promise.all([programmaticPlay(audio), programmaticPlay(video)])
  p.video.readyState = 4;
  p.video.emit('canplay');

  // Before play() settles, a new waiting event fires -> aborts pending play() with AbortError
  p.video.readyState = 2;
  p.video.emit('waiting');

  await new Promise((r) => setTimeout(r, 20));
  assert.equal(p.state.splitBufferingPaused, true);
  assert.equal(p.state.buffering, true);
  assert.equal(p.state.recovering, false);
  assert.equal(p.audio.paused, true);
  assert.equal(p.video.paused, true);
  assert.ok(p.state.recoveryTimer !== null);

  // Subsequent canplay resumes cleanly
  p.video.readyState = 4;
  p.video.emit('canplay');
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(p.state.splitBufferingPaused, false);
  assert.equal(p.state.buffering, false);
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
});

test('#progress change while splitBufferingPaused into readyState < 3 arms recoveryTimer; onTimeUpdate clears immediately when bothReady; trySplitPair pauses on error', async () => {
  let streamCalls = 0;
  const p = queued(
    async (command) => {
      if (command !== 'streams') return;
      streamCalls++;
      if (streamCalls === 1) return streams;
      return {
        muxed: [],
        video_only: [
          { url: 'v-err', mime: 'video/mp4', height: 720 },
          { url: 'v-ok', mime: 'video/mp4', height: 720 },
        ],
        audio: [
          { url: 'a-ok', mime: 'audio/mp4' },
          { url: 'a-err', mime: 'audio/mp4' },
        ],
      };
    },
    { resetReadyStateOnSrc: true }
  );
  await p.playCurrent();
  p.audio.readyState = 4;
  p.video.readyState = 4;
  p.audio.currentTime = 50;

  // Enter splitBufferingPaused and simulate #progress drag -> change into readyState < 3
  p.video.readyState = 2;
  p.video.emit('waiting');
  assert.equal(p.state.splitBufferingPaused, true);

  const progress = p.get('progress');
  progress.value = 500;
  progress.emit('input');
  assert.equal(p.state.seeking, true);
  progress.emit('change');
  assert.equal(p.state.seeking, false);
  assert.equal(p.state.buffering, true);
  assert.equal(p.state.splitBufferingPaused, true);
  assert.ok(p.state.recoveryTimer !== null);

  // Resume cleanly via canplay, then verify onTimeUpdate clears recoveryTimer immediately on first bothReady tick
  p.video.readyState = 4;
  p.video.emit('canplay');
  await new Promise((r) => setImmediate(r));
  assert.equal(p.state.recoveryTimer, null);

  p.audio.currentTime = 50;
  p.audio.emit('timeupdate'); // establishes lastProgressPos = 50 while bothReady
  p.video.readyState = 2;
  p.video.emit('stalled');
  assert.ok(p.state.recoveryTimer !== null);
  p.audio.currentTime = 51;
  p.audio.emit('timeupdate'); // bothReady is false -> must NOT clear recoveryTimer or advance lastProgressPos
  assert.ok(p.state.recoveryTimer !== null);

  // Video recovers and audio ticks by 0.1s (51.1) -> clears recoveryTimer immediately because lastProgressPos stayed at 50
  p.video.readyState = 4;
  p.audio.currentTime = 51.1;
  p.audio.emit('timeupdate');
  assert.equal(p.state.recoveryTimer, null);

  // trySplitPair pauses both elements when error is set after play() resolves
  p.video.playError = (src) => {
    if (src === 'v-err') {
      p.video.error = { code: 2 };
    }
    return null;
  };
  p.audio.playError = (src) => {
    if (src === 'a-err') {
      p.audio.error = { code: 2 };
    }
    return null;
  };
  p.audio.error = { code: 2 };
  p.audio.emit('error');
  await new Promise((r) => setImmediate(r));
  assert.equal(streamCalls, 2);
  assert.equal(p.video.src, 'v-ok');
  assert.equal(p.audio.src, 'a-ok');
  assert.equal(p.audio.paused, false);
  assert.equal(p.video.paused, false);
});
