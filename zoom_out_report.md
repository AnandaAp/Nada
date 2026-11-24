# Stage 5 Zoom Out Review Report: Continuous Stream-Caching Proxy

**Target:** `docs/designs/continuous_stream_caching_proxy.md` (Lines 1–270)  
**Goldfish Context Evaluated:** `src-tauri/src/proxy.rs`, `src-tauri/src/lib.rs`  
**Reviewers:** `egm-zoom-out` (Skeptical Architect), `egm-inventive-catalyst` (TRIZ Contradiction Catalyst)

---

## Reviewer Status

| Reviewer | Status | Focus / Lens |
| :--- | :--- | :--- |
| `egm-zoom-out` | completed | High-altitude architecture, simplicity, YAGNI, Windows concurrency invariants, codebase reuse |
| `egm-inventive-catalyst` | completed | TRIZ inventive contradiction resolution, parameter transformation, system dynamism |

---

## Executive Summary & High-Altitude Assessment

The design document solves the critical YouTube Music 1:04 playback termination bug cleanly without introducing heavyweight runtime sidecars (Deno/V8 via `rustypipe-botguard`), honoring Nada's strict <15 MB footprint constraint. 

From an **altitude and simplicity** perspective (`egm-zoom-out`), moving from fragile multi-part upstream HTTP chunk slicing to a single unsegmented stream (`bytes=0-`) coupled with a local ephemeral file cache aligns perfectly with the problem mechanics. However, the review identified key refinement opportunities regarding **Windows file-sharing concurrency**, **lazy download worker spawning**, and **synchronization primitive consolidation**.

From an **inventive catalyst** perspective (`egm-inventive-catalyst`), the design leverages classic TRIZ principles—specifically **Principle 10 (Preliminary Action)** and **Principle 24 (Intermediary)**—to dissolve the bot mitigation contradiction by moving range slicing from the untrusted public network into a trusted local loopback intermediary.

---

## Prioritized Findings (Blockers 🔴 -> Major 🟡 -> Nits 🟢)

### 1. Windows OS File Sharing & Deletion Locking Violation - Severity: 🔴 Blocker
* **Reviewer Lens:** `egm-zoom-out`
* **Affected Scope:** Lines 79–81, 158–170, 178–179
* **Discrepancy Description**: 
  On Windows, `std::fs::File::create` and `std::fs::OpenOptions::new().write(true)` open files with restrictive sharing flags by default if not carefully parameterized. If the `download_worker` holds an active writable file handle to `{token}.tmp` while `serve` concurrently attempts to open read handles via `std::fs::File::open` for `CacheRangeReader`, Windows can throw `std::io::ErrorKind::PermissionDenied` (`ERROR_SHARING_VIOLATION`). Furthermore, LRU eviction attempting `std::fs::remove_file` while reader handles are still open will fail immediately on Windows unless files are opened with `FILE_SHARE_DELETE`.
* **Rationale & Recommendation**: 
  * Explicitly parameterize both writer and reader file handles with non-exclusive shared read/write/delete permissions.
  * For LRU eviction and cleanup, wrap `remove_file` in a soft-fail handler that ignores in-use errors or defers unlinking until all `Arc<StreamSession>` references drop.

---

### 2. Forward Seek Starvation & WebView Socket Timeout on Long Media - Severity: 🟡 Major
* **Reviewer Lens:** `egm-zoom-out` & `egm-inventive-catalyst`
* **Affected Scope:** Lines 47–51 (CUJ 2), 146–148, 207–209
* **Discrepancy Description**: 
  If a user seeks forward past the downloaded boundary (e.g., jumping to 3:30 of a 4:00 audio track or scrubbing into a 150 MB music video), `CacheRangeReader` will sleep in a loop waiting on `Condvar` until the single sequential `download_worker` catches up from byte 0. On average or slow connections, catching up 15–50 MB can take 5–25 seconds. Chromium/WebKit `<audio>` elements will trigger a media network error or abort the connection if the HTTP 206 stream stalls inside `read()` for longer than the internal socket inactivity timeout.
* **Rationale & Recommendation**: 
  * Audio tracks (typically 3–6 MB total at 128 kbps) download over broadband in <1.5s, but the design should specify a bounded wait threshold: if `pos >= downloaded` and `(pos - downloaded) > FORWARD_SEEK_THRESHOLD` (e.g., > 2 MB ahead), the proxy should log forward-seek catchup latency and return data in non-blocking chunks or handle player reconnect gracefully.

---

### 3. Eager Worker Thread Spawning on `register()` (YAGNI & Socket Waste) - Severity: 🟡 Major
* **Reviewer Lens:** `egm-zoom-out`
* **Affected Scope:** Lines 153–157, 232–234
* **Discrepancy Description**: 
  The design specifies: `StreamProxy::register` creates the session and immediately launches a dedicated `std::thread` executing `download_worker`. In `src-tauri/src/lib.rs`, `register` is called when formatting `StreamsResponse` (which exposes audio, muxed video, and video-only streams). If both audio and video streams are registered, or if tracks are preloaded for playlists/radio, multiple unsegmented background HTTP downloads will immediately start saturating upstream bandwidth and disk I/O, even if the user never presses play on that stream.
* **Rationale & Recommendation**: 
  * **Lazy Downloader Spawning (On-Demand)**: Defer spawning `download_worker` until the first HTTP `GET /s/{token}` request is received by `serve`. 
  * `register()` should merely record the URL and generate the token. Only when the WebView actually requests bytes from the local proxy should the download worker thread be instantiated.

---

### 4. Fragmented Synchronization Primitives in `SharedProgress` - Severity: 🟢 Nit
* **Reviewer Lens:** `egm-zoom-out`
* **Affected Scope:** Lines 103–114, 226–229
* **Discrepancy Description**: 
  `SharedProgress` mixes atomic types with mutexes and an empty `lock: Mutex<()>` paired with `notify: Condvar`.
* **Rationale & Recommendation**: 
  Maintain `downloaded: AtomicU64` for lockless hot-path reads, but group the condition variable with a single `Mutex<Option<String>>` (error/state) to eliminate the redundant empty `Mutex<()>`.

---

### 5. LRU Eviction Under Active Session Playback - Severity: 🟢 Nit
* **Reviewer Lens:** `egm-zoom-out`
* **Affected Scope:** Line 178
* **Discrepancy Description**: 
  The design does not account for the case where an evicted track is currently being read by an active `tiny_http` worker.
* **Rationale & Recommendation**: 
  Only evict sessions whose `Arc::strong_count == 1` (meaning no active reader holds a clone of the session). When evicting, cancel the worker first, then drop the handle before calling `std::fs::remove_file`.

---

## TRIZ Inventive Contradiction Resolution Matrix

| Contradiction | Standard Technical Conflict | TRIZ Inventive Principle Applied | Evaluation of Proposed Design |
| :--- | :--- | :--- | :--- |
| **1. Anti-Bot Enforcement vs. Zero-Binary Footprint** | YouTube CDN blocks non-zero range requests without Botguard PO token. Botguard VM requires heavy JS engine (30MB+). | **Principle 10 (Preliminary Action)** + **Principle 24 (Intermediary)** | **Highly Effective**: The single preliminary unsegmented stream (`Range: bytes=0-`) avoids the bot check entirely; the local HTTP server acts as a trusted intermediary offering local slicing. |
| **2. Fast Scrubbing vs. RAM Memory Limit** | In-memory stream buffer provides zero-latency seeking but risks OOM on video streams (50–300 MB). | **Principle 28 (Replacement of Mechanical System / Fields)** | **Effective**: Delegating buffering to an ephemeral file lets the OS Virtual Memory / Page Cache maintain hot audio bytes in RAM while allowing cold data to flush to disk safely. |
| **3. Real-Time Playback Start vs. Incomplete Download** | WebView expects immediate playback (TTFB < 150ms), but file is not yet fully downloaded. | **Principle 19 (Periodic Action)** + **Principle 15 (Dynamism)** | **Effective with Minor Gap**: Producer/Consumer with `Condvar` notification allows streaming to begin on byte 1. |
| **4. Deep Forward Seek vs. Monolithic Sequential Stream** | User scrubbing forward to minute 3:00 needs instant audio, but stream is downloaded sequentially from 0:00. | **Principle 1 (Segmentation)** vs **Principle 10 (Prior Action)** | **Opportunity for Improvement**: Audio streams download at 10x-30x real-time on broadband. Recommending priority buffer chunking for audio and lazy connection starts. |
