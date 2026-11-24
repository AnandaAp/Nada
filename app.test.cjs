const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function player(invoke = async () => {}, homeInvoke = async () => ({ items: [] })) {
  const elements = new Map();
  class Element {
    constructor(id) {
      this.id = id;
      this.paused = true;
      this.currentTime = 0;
      this.duration = 200;
      this.readyState = 4;
      this.value = 80;
      this.handlers = {};
      this.children = [];
      this.dataset = {};
      this.value = id === 'search' ? '' : 80;
      const classes = new Set();
      this.classList = {
        add(name) { classes.add(name); },
        remove(name) { classes.delete(name); },
        contains(name) { return classes.has(name); },
        toggle(name, force) { const add = force ?? !classes.has(name); if (add) classes.add(name); else classes.delete(name); return add; },
      };
    }
    addEventListener(type, handler) { (this.handlers[type] ??= []).push(handler); }
    emit(type) { for (const handler of this.handlers[type] ?? []) handler({ target: this, type }); }
    append(...items) { this.children.push(...items); }
    appendChild(item) { this.children.push(item); }
    setAttribute(name, value) { this[name] = value; }
    set textContent(value) { this.text = value; this.children = []; }
    get textContent() { return this.text; }
    removeAttribute(name) { delete this[name]; }
    load() {}
    blur() {}
    pause() { this.paused = true; this.emit('pause'); }
    play() { this.paused = false; this.emit('play'); return this.playError ? Promise.reject(this.playError) : Promise.resolve(); }
    canPlayType(mime) { return mime.includes('unsupported') ? '' : 'probably'; }
  }
  const context = vm.createContext({
    console,
    document: {
      getElementById(id) { if (!elements.has(id)) elements.set(id, new Element(id)); return elements.get(id); },
      createElement() { return new Element(''); },
      createTextNode(text) { return text; },
      querySelectorAll() { return []; },
      addEventListener() {},
    },
    window: { __TAURI__: { core: { invoke: (command, args) => command === 'home_section' ? homeInvoke(args) : invoke(command, args) }, event: { listen() {} } } },
    navigator: {},
    localStorage: { getItem() { return null; }, setItem() {} },
    setInterval(fn) { context.sync = fn; },
    setTimeout() { return 1; },
    clearTimeout() {},
  });
  vm.runInContext(fs.readFileSync(`${__dirname}/src/app.js`, 'utf8') + '\n globalThis.api = {state, audio, video, activeEl, pickVideo, playCurrent, togglePlay, seekTo, onEnded, next, prev, toggleRepeat, toggleShuffle, setQueue, updateModeButtonsUI, goHome, doSearch, openCollection, loadHome};', context);
  return { ...context.api, sync: context.sync, get: (id) => elements.get(id) };
}

const streams = {
  muxed: [],
  video_only: [{ url: 'video', mime: 'video/mp4', height: 720 }],
  audio: [{ url: 'audio', mime: 'audio/mp4' }],
};
const track = { id: 'one', name: 'One', artist: 'Artist', video: true };

function queued(invoke) {
  const p = player(invoke);
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
  await p.togglePlay();
  resolveStream(streams);
  await startup;
  assert.equal(p.state.mode, null);
  assert.equal(p.state.wantPlaying, false);
  assert.equal(p.audio.paused, true);
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
