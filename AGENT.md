# AGENT.md — Autonomous AI Agent Guidelines & Architecture Manual

> **Authoritative Operational Manual for AI Agents, Coding Assistants, and Automated Tooling**  
> Target Repository: `Nada` (Ultra-Lightweight Desktop Music & Video Player for Windows)

---

### Language / Bahasa
[English](#english) • [Bahasa Indonesia](#bahasa-indonesia)

---

<a name="english"></a>
## English

### 1. Mission & Operational Boundaries

This document serves as the primary system specification and operational contract for any AI agent, coding assistant, or autonomous tool operating on the Nada codebase.

#### Operational Persona
When modifying or extending this codebase, the agent acts as a **systems-level Rust & high-performance Vanilla Web engineer**:
- Prioritize minimal, surgical, and idiomatic changes over broad rewrites.
- Preserve zero-dependency purity and sub-50MB runtime performance.
- Never introduce speculative abstractions or unnecessary dependencies.
- Every architectural decision must respect the constraints listed below without compromise.

---

### 2. Inviolable Architectural Constraints (The 5 Invariants)

Autonomous agents must **NEVER** violate the following five core invariants under any circumstance:

#### Invariant 1: Window Geometry & Desktop Presence
- **Fixed Width of 430px**: The window width is locked at strictly `430px` logical units (`minWidth: 430`, `maxWidth: 430`). Neither user interactions nor code modifications may allow horizontal resizing.
- **Flexible Height**: The window height is resizable with a minimum of `520px` (`minHeight: 520`) and initial height of `640px`. At runtime, the window height may expand up to the display monitor's usable work area (`monitor.work_area().size.height`).
- **Maximization Disabled**: Maximization is disabled at both configuration (`maximizable: false` in `tauri.conf.json`) and window API levels (`window.set_maximizable(false)` in `src-tauri/src/lib.rs`).
- **Background Persistence via System Tray**: Window closing is intercepted (`WindowEvent::CloseRequested` triggers `api.prevent_close(); window.hide()`). Playback must continue in the background until the user explicitly selects "Keluar" from the tray menu.
- **DPI Scaling Awareness**: The physical window size must always account for monitor DPI scaling (`(430.0 * scale).round()`).

#### Invariant 2: Zero External Binary Policy
- **NO Companion Sidecars**: Strictly prohibited from adding helper sidecar executables such as `rustypipe-botguard.exe`, `yt-dlp.exe`, or `ffmpeg.exe`.
- **NO Bundled Runtimes**: Neither Node.js, Deno, nor Python runtimes may be packaged into the release distribution.
- **Pure Native Execution**: All scraping, network transport, stream caching, and IPC handling must be executed natively in Rust crates (`rustypipe`, `tiny_http`, `reqwest` with `rustls-tls`, `tokio`).
- **Minimal Capabilities**: `src-tauri/capabilities/default.json` grants only essential window and event permissions (`core:default`, `core:event:default`, `core:window:allow-show`, `core:window:allow-hide`, `core:window:allow-set-focus`, `core:window:allow-close`). Do not grant shell execution or generic filesystem permissions.

#### Invariant 3: Continuous Stream-Caching Proxy Protocols
- **Elimination of the 1:04 Cutoff**: YouTube CDN drops unsegmented direct browser connections at roughly 1:04 due to upstream bandwidth throttling and missing Proof-of-Origin tokens.
- **Local Proxy Architecture**: `src-tauri/src/proxy.rs` runs an internal HTTP server bound to `127.0.0.1:0` with 8 persistent worker threads.
- **Single Unsegmented Upstream GET**: The proxy must fetch media streams in sequential 512 KB chunks (`chunk_size = 512 * 1024`) using an official iOS user agent (`com.google.ios.youtube/19.29.1`).
- **Single-Writer Multi-Reader with Condvar**: Chunks are written sequentially to an ephemeral disk cache (`%LOCALAPPDATA%/com.anand.nada/stream_cache/{token}.tmp`). `CacheRangeReader` serves frontend HTTP `Range` requests with `206 Partial Content`. If the frontend reads ahead of the download, it synchronizes via `Condvar` without connection drops.
- **Ephemerality**: Cache files must never accumulate indefinitely. An LRU cache limits active disk temporary files to 10, and all `.tmp` files are cleared on application launch.

#### Invariant 4: Pure Vanilla Web Frontend
- **Zero Framework Rule**: The frontend must remain 100% vanilla JavaScript (ES2022+), standard HTML5, and CSS3. Strictly prohibited from adding React, Vue, Svelte, Angular, Solid, or frontend bundlers (Vite/Webpack) to `src/`.
- **Direct DOM Architecture**: UI state is synchronized directly with DOM nodes in `src/app.js`.
- **Split Stream Synchronization**: When playing adaptive YouTube streams (separate audio and video tracks), `<audio>` is always the master clock. A 500ms heartbeat must keep `<video>` within 300ms of `<audio>`.
- **Concurrency Guards**: All asynchronous operations must increment and verify monotonic transaction IDs (`playbackId`, `viewId`) to prevent out-of-order responses from corrupting UI state.

#### Invariant 5: Design Token Adherence (iLaras v2.0)
- All visual modifications must strictly follow the **iLaras v2.0 Dark Studio Precision** design system defined in `src/styles.css`.
- Must use canonical tokens:
  - Base canvas: `--bg-studio: #111110`
  - Surfaces: `--surface-panel: #181820`, `--surface-card: #22222b`
  - Gamelan brass: `--accent-brass: #d97706`, `--accent-brass-hover: #f59e0b`
  - Liquid glass: `--glass-face-bronze`, `--glass-hairline`, layered rim shadows
  - Typography: `Plus Jakarta Sans` for interface controls; `JetBrains Mono` for duration counters and debug info.

---

### 3. File Layout & Module Responsibilities

| Path | Layer / Module | Primary Responsibility | Key Invariants & Dependencies |
|---|---|---|---|
| `src/index.html` | Frontend Markup | Semantic DOM tree, SVG icon definitions, player dock, video stage, and drawer containers. | No external CDN scripts; clean semantic structure. |
| `src/styles.css` | Frontend Presentation | iLaras v2.0 CSS custom properties, Liquid Glass styling, 430px fixed layout, small-height responsive query. | Strictly adheres to design tokens; flex-column viewport architecture. |
| `src/app.js` | Frontend Controller | Client state machine, DOM event bindings, audio/video synchronization, keyboard shortcuts, Tauri IPC caller. | Pure vanilla ES2022; race condition guards (`playbackId`, `viewId`). |
| `src-tauri/src/main.rs` | Tauri Binary Entry | Application bootstrap, Windows console window suppression (`windows_subsystem = "windows"`). | Release build must suppress terminal window. |
| `src-tauri/src/lib.rs` | Tauri Core & IPC | Rust app setup, runtime window geometry clamping, system tray integration, single instance handler, IPC commands. | Locks physical 430px width using DPI scaling factor; prevents window destruction on close. |
| `src-tauri/src/proxy.rs` | Streaming Proxy | Embedded `tiny_http` server on `127.0.0.1:0`, iOS client stream reader, 512KB chunked disk caching, Condvar sync, Range serving. | Prevents 1:04 cutoff; manages ephemeral `.tmp` files with LRU eviction. |
| `src-tauri/tauri.conf.json` | Tauri Configuration | App metadata, bundle parameters (NSIS target), CSP headers, window size declaration (`430x640`, non-maximizable). | Security CSP; no `externalBin` sidecars allowed. |
| `src-tauri/capabilities/default.json` | Tauri Permissions | Minimal permission manifest for Tauri v2 security model. | Principle of least privilege; no shell or filesystem permissions. |
| `src-tauri/Cargo.toml` | Rust Dependencies | Crate manifests, feature selections, release profile optimizations (`opt-level = "s"`, `lto = true`, `strip = true`). | Minimal dependencies; `reqwest` uses `rustls-tls` to avoid OpenSSL dll dependencies. |
| `app.test.cjs` | Frontend Unit Tests | Node.js mock environment testing player state machines, shuffle preservation, repeat cycling, and stream synchronization. | 24 automated tests; must pass 100% on every change. |
| `.github/workflows/ci.yml` | GitHub Actions CI | Automated linting, test suite execution (24 JS tests, 5 Rust tests), and compilation dry-runs on PRs and pushes to main. | Protects repository health on every commit. |
| `.github/workflows/release.yml` | GitHub Actions Release | Automated packaging and release pipeline for Windows (NSIS installer, portable zip, and SHA256SUMS.txt). Triggered by tag (`v*`) or manual dispatch. | Zero external binary guarantee; signs and publishes official release assets. |

---

### 4. IPC Interface Reference

All frontend-to-backend communication occurs over Tauri v2's IPC bridge (`window.__TAURI__.core.invoke`).

#### 1. `search`
- **Rust Signature**: `search(state: State<'_, AppState>, query: String, kind: String) -> Result<Vec<Track>, String>`
- **Frontend Call**: `invoke("search", { query: "gamelan", kind: "tracks" })`
- **Parameters**:
  - `query` (`String`): Search keywords.
  - `kind` (`String`): Either `"tracks"` (songs) or `"videos"` (music videos).
- **Return Value**: `Vec<Track>`

#### 2. `streams`
- **Rust Signature**: `streams(state: State<'_, AppState>, video_id: String) -> Result<StreamsResponse, String>`
- **Frontend Call**: `invoke("streams", { videoId: "dQw4w9WgXcQ" })`
- **Parameters**:
  - `videoId` (`String`): 11-character YouTube video identifier.
- **Return Value**: `StreamsResponse` containing tokenized local proxy URLs (`http://127.0.0.1:{port}/s/{token}`).

#### 3. `radio`
- **Rust Signature**: `radio(state: State<'_, AppState>, video_id: String) -> Result<Vec<Track>, String>`
- **Frontend Call**: `invoke("radio", { videoId: "dQw4w9WgXcQ" })`
- **Parameters**:
  - `videoId` (`String`): Seed video ID for generating infinite radio continuation tracks.
- **Return Value**: `Vec<Track>`

#### 4. `home_section`
- **Rust Signature**: `home_section(state: State<'_, AppState>, section: String) -> Result<HomeSection, String>`
- **Frontend Call**: `invoke("home_section", { section: "community" })`
- **Parameters**:
  - `section` (`String`): One of `"community"` (trending playlists), `"releases"` (new albums), `"indonesia"` (top local charts), or `"international"` (top global charts).
- **Return Value**: `HomeSection`

#### 5. `collection_tracks`
- **Rust Signature**: `collection_tracks(state: State<'_, AppState>, id: String, kind: String) -> Result<Vec<Track>, String>`
- **Frontend Call**: `invoke("collection_tracks", { id: "VLPL...", kind: "playlist" })`
- **Parameters**:
  - `id` (`String`): Playlist ID or Album browse ID.
  - `kind` (`String`): Either `"playlist"` or `"album"`.
- **Return Value**: `Vec<Track>`

#### 6. `set_tray_title`
- **Rust Signature**: `set_tray_title(app: tauri::AppHandle, title: String)`
- **Frontend Call**: `invoke("set_tray_title", { title: "Song Name - Artist" })`
- **Behavior**: Updates the Windows system tray icon tooltip to `"Nada — {title}"`.

#### 7. System Events
- **`nada://toggle-play`**: Emitted by Rust backend when user clicks "Play / Pause" in the system tray menu. Frontend listens via `window.__TAURI__.event.listen("nada://toggle-play", ...)`.

#### Data Type Schemas (Rust / JSON)
```rust
struct Track {
    id: String,
    name: String,
    artist: String,
    duration: Option<u32>, // seconds
    cover: Option<String>, // thumbnail URL
    video: bool,
}

struct StreamsResponse {
    muxed: Vec<VideoOption>,
    video_only: Vec<VideoOption>,
    audio: Vec<AudioOption>,
    expires_in_seconds: u32,
}

struct AudioOption {
    url: String,     // Local proxy URL (http://127.0.0.1:{port}/s/{token})
    mime: String,    // e.g. "audio/mp4; codecs=\"mp4a.40.2\""
    bitrate: u32,
}

struct VideoOption {
    url: String,     // Local proxy URL
    mime: String,    // e.g. "video/mp4; codecs=\"avc1.4d401f\""
    quality: String, // e.g. "720p"
    height: u32,
}

struct HomeSection {
    items: Vec<HomeItem>,
}

struct HomeItem {
    id: String,
    name: String,
    artist: String,
    cover: Option<String>,
    kind: String,    // "track", "album", or "playlist"
    duration: Option<u32>,
    video: bool,
}
```

---

### 5. Verification & Testing Protocol

Before committing or proposing any code change, an AI agent must complete all stages of the following verification pipeline:

#### Step 1: Frontend Unit Test Verification
Run the 24-assertion JavaScript integration suite:
```powershell
cd C:\Users\anand\StudioProjects\LocalMachine\nada
node app.test.cjs
```
- **Passing Criteria**: Exactly `pass 24, fail 0, cancelled 0, skipped 0`.
- **Coverage**: Tests split media synchronization, MP4 priority, stream buffering, stale promise cancellation, repeat modes, queue preservation during shuffle, video stage toggle, and PiP events.

#### Step 2: Rust Backend Test Verification
Run the Rust unit test suite:
```powershell
cd C:\Users\anand\StudioProjects\LocalMachine\nada\src-tauri
cargo test
```
- **Passing Criteria**: Exactly `5 passed; 0 failed; 0 ignored`.
- **Coverage**: Tests range header parsing, shelf parser error isolation, playlist card sanitization, cache reader chunk crossing, and thread cancellation safety.

#### Step 3: Working Tree Cleanliness Inspection
Confirm no unintended build artifacts or temporary files are untracked:
```powershell
git status --porcelain
```
- Only intentionally modified files should appear in the status output.

---

<a name="bahasa-indonesia"></a>
## Bahasa Indonesia

### 1. Misi & Batasan Operasional

Dokumen ini merupakan spesifikasi acuan dan kontrak operasional utama bagi seluruh AI agent, asisten coding, maupun sistem otomatisasi yang beroperasi pada basis kode Nada.

#### Peran dan Karakter Operasional
Ketika memodifikasi atau mengembangkan basis kode ini, agen bertindak sebagai **Insinyur Sistem Rust & Pengembang Web Vanilla Performa Tinggi**:
- Prioritaskan perubahan minimal, tepat sasaran (*surgical*), dan idiomatik daripada perombakan skala besar.
- Pertahankan kemurnian bebas dependensi eksternal dan konsumsi memori di bawah 50MB RAM.
- Dilarang menambahkan abstraksi spekulatif atau pustaka pihak ketiga yang tidak mutlak dibutuhkan.
- Setiap keputusan teknis harus mematuhi seluruh batasan arsitektur di bawah ini tanpa pengecualian.

---

### 2. Batasan Arsitektur yang Tidak Boleh Dilanggar (5 Invarian Utama)

AI Agent **DILARANG KERAS** melanggar kelima invarian inti berikut dalam situasi apa pun:

#### Invarian 1: Geometri Jendela & Perilaku Desktop
- **Lebar Tetap 430px**: Lebar jendela terkunci secara mutlak pada `430px` (`minWidth: 430`, `maxWidth: 430`). Baik interaksi pengguna maupun kode tidak boleh mengubah ukuran horizontal.
- **Tinggi Fleksibel**: Tinggi jendela dapat diubah ukurannya dengan batas minimal `520px` (`minHeight: 520`) dan tinggi awal `640px`. Saat berjalan, tinggi jendela dapat diperluas hingga batas maksimal area kerja monitor (`monitor.work_area().size.height`).
- **Tombol Maksimalkan Dinonaktifkan**: Fitur maksimalkan dinonaktifkan baik di konfigurasi (`tauri.conf.json`) maupun melalui pemanggilan API jendela Rust (`window.set_maximizable(false)`).
- **Persistensi Latar Belakang di Baki Sistem**: Penutupan jendela dicegat (`CloseRequested` memicu `window.hide()`). Pemutaran audio harus tetap berjalan di latar belakang hingga pengguna memilih "Keluar" dari menu tray.
- **Kesadaran Skala DPI**: Ukuran fisik jendela harus selalu memperhitungkan faktor skala DPI layar (`(430.0 * scale).round()`).

#### Invarian 2: Kebijakan Bebas Biner Eksternal (Zero External Binary Policy)
- **TANPA Sidecar Tambahan**: Dilarang menambahkan berkas biner pendamping seperti `rustypipe-botguard.exe`, `yt-dlp.exe`, atau `ffmpeg.exe`.
- **TANPA Runtime Tambahan**: Tidak boleh menyertakan runtime Node.js, Deno, maupun Python ke dalam installer paket rilis.
- **Eksekusi Native Murni**: Seluruh operasi jaringan, penguraian data, proksi streaming, dan penanganan IPC harus murni dijalankan oleh crate Rust (`rustypipe`, `tiny_http`, `reqwest` dengan `rustls-tls`, `tokio`).
- **Hak Akses Minimal**: Berkas `capabilities/default.json` hanya memberikan izin jendela dan event esensial. Dilarang memberikan izin eksekusi shell atau akses sistem berkas menyeluruh ke frontend.

#### Invarian 3: Protokol Proksi Caching Streaming Kontinu
- **Pemberantasan Masalah Pemutusan 1:04**: CDN YouTube memutuskan koneksi peramban langsung pada detik ke-1:04 akibat pembatasan laju transfer upstream dan ketiadaan token Proof-of-Origin pada permintaan bertahap.
- **Arsitektur Proksi Lokal**: Berkas `src-tauri/src/proxy.rs` menjalankan server HTTP internal pada `127.0.0.1:0` dengan 8 worker thread persisten.
- **Permintaan Upstream Tunggal Tanpa Fragmentasi**: Proksi mengambil aliran media dalam potongan teratur 512 KB (`chunk_size = 512 * 1024`) menggunakan identitas user-agent iOS (`com.google.ios.youtube/19.29.1`).
- **Single-Writer Multi-Reader dengan Sinkronisasi Condvar**: Data ditulis secara berurutan ke berkas cache sementara (`%LOCALAPPDATA%/com.anand.nada/stream_cache/{token}.tmp`). `CacheRangeReader` melayani permintaan HTTP `Range` frontend dengan status `206 Partial Content`. Jika frontend membaca mendahului unduhan, pembacaan menunggu sinyal `Condvar` tanpa memutus sambungan.
- **Pembersihan Cache**: Berkas cache tidak boleh menumpuk. Mekanisme LRU membatasi maksimal 10 berkas sementara aktif, dan semua berkas `.tmp` dibersihkan saat aplikasi pertama kali dijalankan.

#### Invarian 4: Frontend Web Vanilla Tanpa Framework
- **Aturan Bebas Framework**: Frontend harus tetap 100% menggunakan vanilla JavaScript (ES2022+), HTML5 standar, dan CSS3 murni. Dilarang menambahkan React, Vue, Svelte, Angular, Solid, maupun bundler (Vite/Webpack) ke direktori `src/`.
- **Arsitektur DOM Langsung**: Sinkronisasi status antarmuka dikelola langsung pada elemen DOM dalam `src/app.js`.
- **Sinkronisasi Aliran Terpisah (Split Stream)**: Pada tayangan video adaptif di mana aliran audio dan video terpisah, elemen `<audio>` selalu menjadi jam acuan utama (*master clock*). Heartbeat 500ms menjaga selisih waktu `<video>` tidak lebih dari 300ms dari `<audio>`.
- **Perlindungan Balapan Asinkron (*Race Conditions*)**: Seluruh operasi asinkron wajib memvalidasi penghitung ID transaksi (`playbackId`, `viewId`) agar respons jaringan yang usang tidak merusak status antarmuka.

#### Invarian 5: Kepatuhan Sistem Desain iLaras v2.0
- Seluruh modifikasi visual harus mematuhi token desain **iLaras v2.0 Dark Studio Precision** di `src/styles.css`.
- Wajib menggunakan token standar:
  - Kanvas utama: `--bg-studio: #111110`
  - Permukaan panel & kartu: `--surface-panel: #181820`, `--surface-card: #22222b`
  - Kuningan gamelan: `--accent-brass: #d97706`, `--accent-brass-hover: #f59e0b`
  - Liquid glass: `--glass-face-bronze`, garis tepi halus, dan bayangan rim berlapis.
  - Tipografi: `Plus Jakarta Sans` untuk teks kontrol; `JetBrains Mono` untuk indikator waktu dan durasi.

---

### 3. Peta Tata Letak Berkas & Tanggung Jawab Modul

| Berkas | Lapisan / Modul | Tanggung Jawab Utama |
|---|---|---|
| `src/index.html` | Struktur Antarmuka | Pohon DOM semantik, ikon SVG, kontrol pemutar, panggung video, dan drawer antrean. |
| `src/styles.css` | Gaya Presentasi | Token desain iLaras v2.0, tata letak tetap 430px, media query responsif ketinggian rendah. |
| `src/app.js` | Kontroler Frontend | State machine klien, event listener, sinkronisasi audio/video, pintasan keyboard, pemanggil IPC. |
| `src-tauri/src/main.rs` | Titik Masuk Rust | Inisialisasi aplikasi, penonaktifan jendela konsol Windows (`windows_subsystem = "windows"`). |
| `src-tauri/src/lib.rs` | Inti Tauri & IPC | Konfigurasi aplikasi, penguncian dimensi jendela fisik, integrasi baki sistem, penangan IPC. |
| `src-tauri/src/proxy.rs` | Proksi Streaming | Server HTTP `tiny_http`, pengunduh klien iOS, caching disk 512KB, sinkronisasi Condvar. |
| `src-tauri/tauri.conf.json` | Konfigurasi Tauri | Metadata aplikasi, deklarasi dimensi jendela (430x640), target installer NSIS, header keamanan CSP. |
| `src-tauri/capabilities/default.json` | Hak Akses Tauri | Deklarasi hak akses minimal untuk model keamanan Tauri v2. |
| `src-tauri/Cargo.toml` | Dependensi Rust | Daftar pustaka Rust, profil optimasi rilis (`opt-level = "s"`, `lto = true`, `strip = true`). |
| `app.test.cjs` | Pengujian Unit Frontend | Lingkungan uji mock Node.js untuk 24 skenario status pemutar, acak, pengulangan, dan sinkronisasi. |
| `.github/workflows/ci.yml` | GitHub Actions CI | Otomasi pengujian (24 tes JS, 5 tes Rust) dan validasi kompilasi pada pull request serta push ke main. |
| `.github/workflows/release.yml` | GitHub Actions Release | Pipeline pembuatan paket rilis Windows otomatis (installer NSIS, portable zip, dan SHA256SUMS.txt) via git tag (`v*`) atau pemicu manual. |

---

### 4. Referensi Antarmuka IPC

Komunikasi frontend ke backend memanfaatkan jembatan IPC Tauri v2 (`window.__TAURI__.core.invoke`):
1. `search(query: String, kind: String) -> Result<Vec<Track>, String>`: Mencari lagu (`"tracks"`) atau video (`"videos"`).
2. `streams(video_id: String) -> Result<StreamsResponse, String>`: Mengambil tautan proksi lokal untuk audio dan video.
3. `radio(video_id: String) -> Result<Vec<Track>, String>`: Mengambil rekomendasi kelanjutan stasiun radio tanpa henti.
4. `home_section(section: String) -> Result<HomeSection, String>`: Mengambil konten beranda (`"community"`, `"releases"`, `"indonesia"`, `"international"`).
5. `collection_tracks(id: String, kind: String) -> Result<Vec<Track>, String>`: Mengambil daftar lagu dari album atau playlist.
6. `set_tray_title(title: String)`: Memperbarui teks keterangan (tooltip) pada ikon baki sistem.
7. Event `nada://toggle-play`: Dipancarkan oleh baki sistem saat menu "Play / Pause" ditekan.

---

### 5. Protokol Verifikasi & Pengujian

Sebelum mengajukan perubahan apa pun, AI agent wajib menjalankan dan meloloskan seluruh tahapan verifikasi berikut:

#### 1. Verifikasi Pengujian Frontend
```powershell
cd C:\Users\anand\StudioProjects\LocalMachine\nada
node app.test.cjs
```
- **Kriteria Lolos**: Wajib `pass 24, fail 0`.

#### 2. Verifikasi Pengujian Unit Backend Rust
```powershell
cd C:\Users\anand\StudioProjects\LocalMachine\nada\src-tauri
cargo test
```
- **Kriteria Lolos**: Wajib `5 passed; 0 failed`.

#### 3. Pemeriksaan Kebersihan Status Git
```powershell
git status --porcelain
```
- Pohon kerja harus bersih, hanya memuat berkas yang memang ditugaskan untuk diubah.
