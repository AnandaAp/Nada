# Nada (nada)

> **Ultra-Lightweight YouTube Music & Video Desktop Player for Windows**  
> Built with Tauri v2 (Rust backend) and vanilla HTML5 / CSS3 / ES2022 (frontend).

---

### Language / Bahasa
[English](#english) • [Bahasa Indonesia](#bahasa-indonesia)

---

<a name="english"></a>
## English

### 1. Overview
**Nada** is an ultra-lightweight, ad-free desktop music and video player designed specifically for Windows 10 and 11. Powered by **Tauri v2** and Microsoft Edge **WebView2**, Nada brings YouTube Music streaming directly to your desktop with negligible resource consumption:
- **Sub-100ms startup**: Launches near-instantly without heavyweight framework overhead.
- **Ultra-low memory footprint**: Typically runs under **50 MB RAM** during active audio playback.
- **No login or tracking**: Stream tracks, albums, community playlists, and artist radios without logging into third-party accounts.
- **Desktop audio laboratory**: Combines high-fidelity streaming with a refined studio aesthetic inspired by acoustic precision and Javanese cultural craftsmanship.

---

### 2. Key Features

- **YouTube Music Streaming**: Seamless search and playback across tracks, albums, community playlists, and infinite radio streams directly from YouTube Music.
- **Continuous Stream Caching (No 1:04 Cutoff)**: An internal Rust streaming proxy eliminates the notorious 1:04 playback cutoff caused by YouTube CDN throttling and missing proof-of-origin tokens on unsegmented connections.
- **Picture-in-Picture (PiP)**: Watch video streams in a persistent floating window via either the dedicated floating PiP button or native HTML5 `<video>` context menu controls.
- **Audio & Video Stage Modes**: Effortlessly switch between audio-only playback and an integrated 16:9 synchronized video stage. When switching modes, playback position and paused/playing states are preserved automatically.
- **Repeat Modes (Off / All / One)**: Cycle repeat modes with visual badge feedback (`1` badge on single track repeat). Single repeat loops the current track; full queue repeat wraps around seamlessly.
- **Smart Queue-Preserving Shuffle**: Shuffling preserves the currently playing song in place while randomizing upcoming songs. Disabling shuffle restores the original queue order without interrupting playback.
- **System Tray Integration**:
  - Background audio playback persists when the window is closed (`WindowEvent::CloseRequested` minimizes to tray).
  - Context menu with `Buka Nada`, `Play / Pause`, and `Keluar`.
  - Left-clicking the tray icon restores and focuses the window.
  - Dynamic tray tooltip updates with track title and artist.
- **Keyboard Shortcuts**:
  | Shortcut | Function | Context |
  |---|---|---|
  | `Space` | Toggle Play / Pause | Global (suppressed during text input) |
  | `ArrowRight` | Seek +5 seconds forward | Global (suppressed during text input) |
  | `ArrowLeft` | Seek -5 seconds backward | Global (suppressed during text input) |
  | `r` / `R` | Cycle Repeat (Off → All → One → Off) | Global (suppressed during text input) |
  | `s` / `S` | Toggle Shuffle (On / Off) | Global (suppressed during text input) |
  | `Enter` | Submit search query | When search input is focused |
  | `Enter` / `Space` | Select and play track | When track item is focused (`role="button"`) |

---

### 3. Window Constraints & Geometry

Nada is purposefully designed with a compact, mobile-inspired form factor:
- **Fixed Width**: Locked strictly to **430px** (`minWidth: 430`, `maxWidth: 430`). Horizontal resizing is prohibited to ensure pixel-perfect layout integrity.
- **Flexible Height**: Resizable with a minimum height of **520px** (`minHeight: 520`) and an initial launch height of **640px**, expanding up to the maximum display work area.
- **Maximization Disabled**: The window maximize button is disabled (`maximizable: false`) to preserve its identity as a companion utility.
- **DPI-Aware Physical Size Enforcement**: On launch and single-instance activation, physical dimensions are dynamically calculated using the display's scale factor (`(430.0 * scale).round()`) and centered on screen.
- **Compact Viewport Responsive Tuning**: At heights below 560px, `@media (max-height: 560px)` compresses the video viewport to `28vh` to keep player controls, track metadata, and navigation elements accessible without scrolling.

---

### 4. Architecture & Zero External Binary Policy

```
┌────────────────────────────────────────────────────────┐
│               Microsoft Edge WebView2                  │
│       Vanilla HTML5 / CSS3 / ES2022 (src/)             │
│   <audio> (Audio Master)  /  <video> (Stage / PiP)     │
└──────────────────────────┬─────────────────────────────┘
                           │ IPC invoke / events
┌──────────────────────────▼─────────────────────────────┐
│                 Tauri v2 Core (Rust)                   │
│   src-tauri/src/lib.rs (Window, Tray, RustyPipe IPC)   │
└──────────────────────────┬─────────────────────────────┘
                           │ HTTP 127.0.0.1:{ephemeral}
┌──────────────────────────▼─────────────────────────────┐
│           Local Stream Proxy (src-tauri/src/proxy.rs)  │
│  - 8-thread tiny_http server                           │
│  - Single upstream GET with iOS Client User-Agent      │
│  - 512 KB chunked pre-buffering                        │
│  - Ephemeral disk cache (%LOCALAPPDATA%/stream_cache/) │
│  - Condvar-synchronized CacheRangeReader (RFC 206)     │
└────────────────────────────────────────────────────────┘
```

#### Zero External Binary Policy
Nada strictly enforces a **Zero External Binary** policy:
- **NO companion sidecar executables**: Rejected `rustypipe-botguard`, `yt-dlp`, or `ffmpeg`.
- **NO bundled JavaScript runtimes**: No Node.js or Deno runtimes are packaged with the distribution binary.
- **Pure native stack**: Built exclusively using standard Rust crates (`tauri`, `rustypipe`, `tiny_http`, `reqwest` with `rustls-tls`, `tokio`) and the operating system's native WebView2 runtime.
- **Minimal attack surface**: Tauri capabilities restrict permissions to essential window and event management (`core:default`, `core:window:allow-show`, `core:window:allow-hide`, `core:window:allow-set-focus`). No arbitrary shell execution or filesystem access is exposed to the frontend.

#### Continuous Stream-Caching Proxy
Direct streaming from YouTube / Googlevideo CDN endpoints frequently terminates at approximately **1:04** due to upstream TCP throttling and missing Proof-of-Origin tokens on unsegmented range requests.

Nada resolves this seamlessly:
1. **iOS Client Extraction**: Uses `ClientType::Ios` (`com.google.ios.youtube/19.29.1`) to obtain high-bitrate media URLs without web-client cipher throttling.
2. **Deterministic 512 KB Chunking**: An internal 8-worker `tiny_http` server on `127.0.0.1:0` downloads the stream in sequential 512 KB chunks with exponential backoff retries.
3. **Ephemeral Disk Buffer**: Chunks are flushed sequentially to `%LOCALAPPDATA%\com.anand.nada\stream_cache\{token}.tmp`.
4. **Condvar-Synchronized Range Serving**: The custom `CacheRangeReader` responds to WebView2 requests with standard `206 Partial Content` headers. If the player reads ahead of the network download, threads synchronize on a `Condvar` without dropping or stalling the connection.
5. **Automatic Cleanup**: Inactive cache files are evicted using an LRU policy (maximum 10 active disk cache files), and all temporary cache files are purged automatically upon application launch.

---

### 5. Media Format & Codec Support

Nada dynamically queries native WebView2 capabilities via `HTMLMediaElement.canPlayType()`:
- **Audio Formats**:
  - **AAC (`audio/mp4; codecs="mp4a.40.2"`)**: Primary audio format; delivers universal hardware acceleration and crystal-clear acoustic fidelity.
  - **Opus (`audio/webm; codecs="opus"`)**: Supported secondary stream candidate.
- **Video Formats**:
  - **H.264 / AVC1 (`video/mp4; codecs="avc1.*"`)**: Primary video format; full GPU acceleration across Windows installations.
  - **VP9 (`video/webm; codecs="vp9"`)**: Supported secondary format.
  - **Resolution Cap**: Video playback is capped at **720p** for optimal desktop efficiency and low memory consumption.
- **Split Stream Synchronization**:
  - For high-definition adaptive YouTube streams where audio and video are delivered separately, Nada routes the audio stream to `<audio>` (the master clock) and the video stream to `<video>` (muted).
  - A 500ms synchronization heartbeat monitors drift; if drift exceeds 300ms, the video timeline is automatically nudged back into phase with the audio master.
  - If either stream buffers, both pause simultaneously and resume together once `readyState >= 3`.

---

### 6. Design System: iLaras v2.0 Dark Studio Precision

Nada's visual language, **iLaras v2.0 Dark Studio Precision**, merges an acoustic engineering laboratory aesthetic with timeless Javanese cultural heritage:

- **Canvas & Surfaces**:
  - Canvas: Deep acoustic black `--bg-studio: #111110`
  - Surfaces: `--surface-panel: #181820`, `--surface-card: #22222b`, `--surface-card-hover: #2a2a34`
  - Subtle borders: `--border-subtle: #32323e`, `--border-strong: #464654`
- **Gamelan Brass Accents**:
  - Warm brass tones: `--accent-brass: #d97706`, `--accent-brass-hover: #f59e0b`, `--accent-brass-glow: rgba(245, 158, 11, 0.18)`
  - Radiant gold: `--accent-gold: #f59e0b`, `--accent-gold-dark: #b45309`
- **Liquid Glass Elevation**:
  - Multi-stop specular gradients (`--glass-face-bronze`), hairline borders (`rgba(244, 239, 230, 0.08)`), and layered inset rim shadows.
  - Floating PiP button and queue drawer feature backdrop blur filters (`backdrop-filter: blur(8px)`).
- **Acoustic Status Indicators (Slendro / Pelog metaphor)**:
  - In-Tune: Emerald Green `--status-in-tune: #10b981`
  - Sharp: Amber `--status-sharp: #f59e0b`
  - Flat: Ruby Red `--status-flat: #ef4444`
- **Typography**:
  - Primary UI: `Plus Jakarta Sans`, system-ui, -apple-system, sans-serif
  - Timestamps & Engineering Data: `JetBrains Mono`, monospace
- **Micro-Architectural Background**:
  - `.app-backdrop` renders a subtle 48px × 48px laboratory grid pattern (`rgba(244, 239, 230, 0.035)`).

---

### 7. Installation, Build & Run Instructions

#### Prerequisites
- **Operating System**: Windows 10 or Windows 11 (64-bit)
- **WebView2**: Microsoft Edge WebView2 Evergreen Runtime (pre-installed on Windows 10/11)
- **Rust Toolchain**: Rust 1.75+ (2021 edition) and Cargo (`rustup default stable`)
- **Node.js**: Node.js 18+ and npm (used strictly for CLI tooling and development workflow)

#### 1. Clone & Install Dependencies
```bash
git clone https://github.com/AnandaAp/Nada.git
cd nada
npm install
```

#### 2. Run in Development Mode
Launches the development window with hot-reloading for frontend assets:
```bash
npm run dev
# or directly via Tauri CLI:
cargo tauri dev
```

#### 3. Build Production Executable / Installer
Produces an optimized, stripped, standalone NSIS installer in `src-tauri/target/release/bundle/nsis/`:
```bash
npm run build
# or directly via Tauri CLI:
cargo tauri build
```
*The release build automatically applies link-time optimization (`lto = true`), symbol stripping (`strip = true`), and size optimization (`opt-level = "s"`).*

#### 4. Run Test Suites
Both the frontend and backend include automated test suites:
```bash
# Run JavaScript frontend integration tests (24 assertions)
node app.test.cjs
# or via npm script:
npm test

# Run Rust backend unit tests (5 assertions)
cargo test --manifest-path src-tauri/Cargo.toml
```

#### 5. Automated CI/CD & GitHub Releases
Nada includes automated GitHub Actions workflows:
- **Continuous Integration (`.github/workflows/ci.yml`)**: Automatically validates JavaScript unit tests, Rust tests, and compilation checks on pull requests and pushes to `main`.
- **Release Automation (`.github/workflows/release.yml`)**: Automatically builds, packages, and publishes a new GitHub Release with Windows NSIS installer (`.exe`), standalone portable package (`.zip`), and `SHA256SUMS.txt`.
  - Trigger by pushing a git tag:
    ```bash
    git tag v0.1.0
    git push origin v0.1.0
    ```
  - Or trigger manually via the GitHub Actions UI tab (`workflow_dispatch`).

---

<a name="bahasa-indonesia"></a>
## Bahasa Indonesia

### 1. Ikhtisar
**Nada** adalah pemutar musik dan video desktop yang sangat ringan, cepat, dan bebas iklan, dirancang khusus untuk Windows 10 dan 11. Ditenagai oleh **Tauri v2** dan **Microsoft Edge WebView2**, Nada menghadirkan streaming YouTube Music langsung ke desktop Anda dengan efisiensi sistem yang luar biasa:
- **Waktu Mulai Instan (<100ms)**: Terbuka dalam sekejap tanpa beban runtime framework JavaScript yang berat.
- **Konsumsi Memori Sangat Rendah (<50 MB RAM)**: Berjalan lancar pada pemakaian memori di bawah 50 MB selama pemutaran audio aktif.
- **Tanpa Login & Tanpa Pelacakan**: Putar lagu, album, daftar putar komunitas, dan radio artis tanpa perlu masuk ke akun pihak ketiga.
- **Laboratorium Akustik Desktop**: Menggabungkan ketelitian pemutaran audio berkualitas tinggi dengan estetika visual presisi studio bertema warisan budaya Jawa.

---

### 2. Fitur Utama

- **Streaming YouTube Music**: Pencarian instan dan pemutaran lagu, album, daftar putar komunitas (*community playlists*), dan stasiun radio tanpa jeda iklan.
- **Continuous Stream Caching (Bebas Pemutusan 1:04)**: Proksi streaming internal Rust meniadakan masalah klasik pemutusan pemutaran pada menit 1:04 yang disebabkan oleh pembatasan CDN YouTube.
- **Picture-in-Picture (PiP)**: Nikmati tayangan video dalam jendela mengambang fleksibel melalui tombol PiP pada video stage maupun menu konteks native HTML5 `<video>`.
- **Mode Panggung Audio & Video**: Transisi instan antara mode dengar audio murni dan panggung video 16:9 tersinkronisasi. Posisi putar serta status putar/jeda tetap dipertahankan secara utuh saat berpindah mode.
- **Mode Pengulangan (Mati / Semua / Satu Lagu)**: Siklus mode pengulangan dilengkapi lencana status (`1` untuk pengulangan satu lagu). Pengulangan satu lagu memutar ulang lagu aktif secara otomatis; pengulangan semua lagu mengulang antrean dari awal secara mulus.
- **Acak Cerdas Penjaga Antrean (Smart Shuffle)**: Mode acak mempertahankan lagu yang sedang diputar di posisi semula sambil mengacak sisa antrean berikutnya. Menonaktifkan acak akan memulihkan urutan antrean asli tanpa menghentikan pemutaran.
- **Integrasi Baki Sistem (System Tray)**:
  - Musik tetap berjalan di latar belakang saat jendela ditutup (`CloseRequested` otomatis menyembunyikan jendela ke tray).
  - Menu konteks baki sistem lengkap: `Buka Nada`, `Play / Pause`, dan `Keluar`.
  - Klik kiri ikon baki sistem akan mengembalikan dan memfokuskan jendela aplikasi ke layar depan.
  - Tooltip baki sistem diperbarui secara dinamis menampilkan judul lagu dan artis.
- **Pintasan Keyboard**:
  | Pintasan | Aksi | Konteks |
  |---|---|---|
  | `Spasi` | Putar / Jeda (Play/Pause) | Global (diabaikan saat mengetik) |
  | `Panah Kanan` | Lompat maju +5 detik | Global (diabaikan saat mengetik) |
  | `Panah Kiri` | Lompat mundur -5 detik | Global (diabaikan saat mengetik) |
  | `r` / `R` | Siklus Ulangi (Mati → Semua → Satu → Mati) | Global (diabaikan saat mengetik) |
  | `s` / `S` | Beralih Acak / Shuffle (Aktif / Mati) | Global (diabaikan saat mengetik) |
  | `Enter` | Kirim pencarian | Saat kotak pencarian fokus |
  | `Enter` / `Spasi` | Pilih dan putar lagu | Saat elemen lagu fokus (`role="button"`) |

---

### 3. Batasan Dimensi Jendela & Geometri

Nada dirancang secara cermat dengan form factor ringkas terinspirasi dari antarmuka perangkat bergerak:
- **Lebar Tetap**: Terkunci secara mutlak pada **430px** (`minWidth: 430`, `maxWidth: 430`). Pengubahan ukuran horizontal dinonaktifkan agar integritas visual tetap presisi.
- **Tinggi Fleksibel**: Dapat diubah ukurannya dengan batas minimal **520px** (`minHeight: 520`) dan tinggi peluncuran awal **640px**, mampu diperluas hingga batas maksimal area kerja monitor.
- **Tombol Maksimalkan Dinonaktifkan**: Tombol maksimalkan jendela dinonaktifkan (`maximizable: false`) untuk mempertahankan identitasnya sebagai widget pendamping desktop yang praktis.
- **Penerapan Skala DPI Fisik**: Saat aplikasi dimulai atau dipanggil kembali melalui instance tunggal, ukuran fisik dihitung secara dinamis terhadap faktor skala monitor (`(430.0 * scale).round()`) dan diposisikan tepat di tengah layar.
- **Responsif Viewport Rendah**: Pada ketinggian jendela di bawah 560px, `@media (max-height: 560px)` memadatkan tinggi video menjadi `28vh` agar kontrol pemutar, informasi lagu, dan tab pencarian tetap dapat diakses tanpa terpotong.

---

### 4. Arsitektur & Kebijakan Bebas Biner Eksternal

#### Kebijakan Bebas Biner Eksternal (Zero External Binary Policy)
Nada beroperasi dengan prinsip integritas biner murni:
- **TANPA proses sidecar eksternal**: Menolak penggunaan `rustypipe-botguard`, `yt-dlp`, atau `ffmpeg`.
- **TANPA runtime JavaScript tambahan**: Tidak menyertakan Node.js maupun Deno di dalam paket rilis installer.
- **Tumpukan Native Murni**: Dibangun menggunakan pustaka Rust pilihan (`tauri`, `rustypipe`, `tiny_http`, `reqwest` dengan `rustls-tls`, `tokio`) serta runtime Microsoft Edge WebView2 yang telah terpasang di sistem operasi Windows.
- **Permukaan Keamanan Minimal**: File kemampuan Tauri membatasi izin hanya pada pengelolaan jendela dan event dasar. Tidak ada izin eksekusi shell maupun akses sistem berkas bebas yang diekspos ke frontend.

#### Proksi Streaming Lokal (Continuous Stream-Caching Proxy)
Pemutaran langsung dari CDN YouTube sering kali terputus pada detik ke-**1:04** akibat pembatasan koneksi TCP upstream dan ketiadaan token Proof-of-Origin pada permintaan Range yang terfragmentasi.

Nada mengatasi hal ini secara menyeluruh:
1. **Penyamaran Klien iOS**: Memanfaatkan `ClientType::Ios` (`com.google.ios.youtube/19.29.1`) untuk memperoleh tautan media berkecepatan tinggi tanpa hambatan cipher throttling.
2. **Chunking Terstruktur 512 KB**: Server HTTP internal berbasis `tiny_http` pada port dinamis `127.0.0.1:0` dengan 8 worker thread mengunduh aliran data secara bertahap dalam ukuran 512 KB dengan mekanisme *exponential retry*.
3. **Penyimpanan Cache Sementara di Disk**: Potongan data ditulis secara berurutan ke berkas sementara di `%LOCALAPPDATA%\com.anand.nada\stream_cache\{token}.tmp`.
4. **Sinkronisasi Condvar pada Range Reader**: Komponen `CacheRangeReader` melayani permintaan streaming WebView2 dengan status standar `206 Partial Content`. Apabila pemutar membaca lebih cepat daripada unduhan jaringan, thread membaca akan menunggu sinyal `Condvar` tanpa memutuskan koneksi.
5. **Pembersihan Otomatis**: Berkas cache yang tidak aktif dibatasi maksimal 10 berkas melalui mekanisme LRU, dan seluruh berkas cache dibersihkan total setiap kali aplikasi dibuka kembali.

---

### 5. Dukungan Format Media & Kodek

Nada secara dinamis menegosiasikan kemampuan WebView2 menggunakan metode `HTMLMediaElement.canPlayType()`:
- **Format Audio**:
  - **AAC (`audio/mp4; codecs="mp4a.40.2"`)**: Format audio utama; mendukung akselerasi perangkat keras menyeluruh di Windows dengan kejernihan akustik maksimal.
  - **Opus (`audio/webm; codecs="opus"`)**: Format cadangan yang didukung penuh.
- **Format Video**:
  - **H.264 / AVC1 (`video/mp4; codecs="avc1.*"`)**: Format video utama dengan akselerasi GPU native.
  - **VP9 (`video/webm; codecs="vp9"`)**: Format video alternatif.
  - **Pembatasan Resolusi 720p**: Resolusi video dibatasi maksimal 720p untuk menjaga efisiensi daya dan memori.
- **Sinkronisasi Aliran Terpisah (Split Stream)**:
  - Pada format video resolusi tinggi di mana YouTube memisahkan aliran gambar dan suara, Nada menyalurkan audio ke elemen `<audio>` (sebagai jam induk/master clock) dan video tanpa suara ke elemen `<video>`.
  - Heartbeat berkala (500ms) memantau selisih waktu; jika terjadi pergeseran lebih dari 300ms, waktu video diselaraskan kembali ke audio master.
  - Apabila salah satu media mengalami buffering, kedua elemen dijeda serentak dan kembali diputar bersamaan setelah status siap (`readyState >= 3`).

---

### 6. Sistem Desain: iLaras v2.0 Dark Studio Precision

Bahasa visual Nada, **iLaras v2.0 Dark Studio Precision**, memadukan nuansa laboratorium akustik presisi dengan filosofi seni gamelan Jawa:

- **Kanvas & Permukaan**:
  - Kanvas: Hitam akustik pekat `--bg-studio: #111110`
  - Panel & Kartu: `--surface-panel: #181820`, `--surface-card: #22222b`, `--surface-card-hover: #2a2a34`
  - Garis batas halus: `--border-subtle: #32323e`, `--border-strong: #464654`
- **Aksen Kuningan Gamelan**:
  - Warna kuningan hangat: `--accent-brass: #d97706`, `--accent-brass-hover: #f59e0b`, pendaran brass `--accent-brass-glow: rgba(245, 158, 11, 0.18)`
  - Emas berkilau: `--accent-gold: #f59e0b`, `--accent-gold-dark: #b45309`
- **Permukaan Liquid Glass**:
  - Gradien pantulan cahaya (`--glass-face-bronze`), garis tepi super tipis (`rgba(244, 239, 230, 0.08)`), serta efek kedalaman rim lighting berlapis.
  - Tombol PiP mengambang dan panel antrean menggunakan efek kaca buram (`backdrop-filter: blur(8px)`).
- **Indikator Nada Akustik (Metafora Laras Slendro / Pelog)**:
  - Tepat Nada (*In-Tune*): Hijau Zamrud `--status-in-tune: #10b981`
  - Terlalu Tinggi (*Sharp*): Amber `--status-sharp: #f59e0b`
  - Terlalu Rendah (*Flat*): Merah Delima `--status-flat: #ef4444`
- **Tipografi**:
  - Antarmuka Utama: `Plus Jakarta Sans`, sans-serif
  - Penunjuk Waktu & Metadata: `JetBrains Mono`, monospace
- **Latar Belakang Arsitektural**:
  - `.app-backdrop` menampilkan kisi-kisi laboratorium berukuran 48px × 48px (`rgba(244, 239, 230, 0.035)`).

---

### 7. Panduan Instalasi, Pengembangan & Kompilasi

#### Prasyarat Sistem
- **Sistem Operasi**: Windows 10 atau Windows 11 (64-bit)
- **WebView2**: Microsoft Edge WebView2 Evergreen Runtime (bawaan Windows 10/11)
- **Kompiler Rust**: Rust versi 1.75+ dan Cargo (`rustup default stable`)
- **Node.js**: Node.js versi 18+ dan npm (hanya untuk menjalankan antarmuka baris perintah Tauri saat pengembangan)

#### 1. Kloning Repositori & Pasang Dependensi
```bash
git clone https://github.com/AnandaAp/Nada.git
cd nada
npm install
```

#### 2. Menjalankan Mode Pengembangan
Membuka aplikasi dengan pembaruan langsung (hot-reload):
```bash
npm run dev
# atau langsung menggunakan Tauri CLI:
cargo tauri dev
```

#### 3. Membuat Berkas Rilis Installer
Menghasilkan berkas installer mandiri NSIS yang terkompresi optimal di `src-tauri/target/release/bundle/nsis/`:
```bash
npm run build
# atau langsung menggunakan Tauri CLI:
cargo tauri build
```
*Konfigurasi rilis secara otomatis menerapkan optimasi ukuran biner (`opt-level = "s"`), penghapusan simbol debug (`strip = true`), dan LTO penuh (`lto = true`).*

#### 4. Menjalankan Pengujian Otomatis
```bash
# Menjalankan pengujian integrasi frontend JavaScript (24 pengujian)
node app.test.cjs
# atau melalui skrip npm:
npm test

# Menjalankan pengujian unit backend Rust (5 pengujian)
cargo test --manifest-path src-tauri/Cargo.toml
```

#### 5. Otomasi CI/CD & Rilis GitHub Actions
Nada dilengkapi alur kerja otomatis GitHub Actions:
- **Continuous Integration (`.github/workflows/ci.yml`)**: Menguji unit test JavaScript, test backend Rust, dan validasi kompilasi secara otomatis di setiap pull request dan push ke `main`.
- **Automated Release (`.github/workflows/release.yml`)**: Mengompilasi dan memublikasikan rilis GitHub otomatis yang memuat installer NSIS Windows (`.exe`), paket portabel mandiri (`.zip`), dan `SHA256SUMS.txt`.
  - Dapat dipicu dengan membuat tag git versi:
    ```bash
    git tag v0.1.0
    git push origin v0.1.0
    ```
  - Atau dipicu secara manual melalui tab Actions di antarmuka GitHub (`workflow_dispatch`).

---

### Lisensi / License
MIT License. Dibuat dengan presisi untuk pengalaman mendengarkan musik yang tenang, cepat, dan elegan.
