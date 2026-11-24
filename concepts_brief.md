# Nada Continuous Stream-Caching Proxy & 1:04 Playback Fix

* **Author:** anlian
* **Date:** 2026-11-23

## Overview

In Nada, media playback halts consistently at the 1:04 mark due to YouTube CDN's bot mitigation. The proxy in `src-tauri/src/proxy.rs` breaks media requests into sequential 1 MiB chunks (`CHUNK = 1024 * 1024`). The first chunk (`bytes=0-1048575`) corresponds to ~64 seconds of audio and succeeds. However, subsequent range requests (`bytes=1048576-2097151`) are rejected with `HTTP 403 Forbidden` because YouTube enforces Proof-of-Origin (PO) tokens on non-zero range requests for web/iOS clients.

Conversely, requesting an unsegmented stream from byte 0 (`Range: bytes=0-` or full GET) downloads the complete track payload without restriction. This architecture replaces the chunked proxy with a zero-external-binary continuous stream-caching proxy in Rust. When a stream is requested, the internal proxy initiates a single background GET from byte 0 into an ephemeral disk cache (`stream_cache/*.tmp`) and serves arbitrary client HTTP Range requests (including forward and backward seeks) directly from this local buffer.

## Analysis

### 1. Concurrency & Synchronization Model
* **Thread Interlocking**: `tiny_http` runs 8 worker threads for client reads, while a dedicated background thread handles upstream streaming.
* **Progress Tracking**: A shared `SharedProgress` structure tracks `downloaded: AtomicU64`, `total: Mutex<Option<u64>>`, and `is_complete: AtomicBool`.
* **Reader Coordination**: When a client requests byte offset `pos >= downloaded`, the reader thread waits on a `Condvar` with a short timeout. As the background writer flushes chunks (64 KB) to disk, it updates `downloaded` and signals the `Condvar`.
* **Avoiding Premature EOF**: In standard `std::io::Read`, reaching the physical end of a file returns `Ok(0)`. If a reader reaches the current download boundary before completion, returning `Ok(0)` would signal `tiny_http` to terminate the HTTP response. The custom reader blocks until more bytes arrive or the download legitimately finishes.

### 2. Disk & Memory Economics
* **Virtual Memory Utilization**: Writing to a temp file leverages the operating system page cache, keeping actively read audio segments in memory without manual ring-buffer allocations.
* **Storage Footprint**: An audio track consumes 3–8 MB; a 720p video consumes 30–150 MB. Disk-backed caching eliminates OOM risks on memory-constrained systems.
* **Lifecycle & Garbage Collection**: Temp files are stored in `storage_dir.join("stream_cache")`. Orphaned files are purged at startup. Individual session files are marked for deletion when cancelled or replaced.

### 3. Seek Behavior & User Experience
* **Backward Seeks**: Instantaneous, served at local SSD read speeds.
* **Forward Seeks Within Downloaded Range**: Instantaneous with zero network latency.
* **Forward Seeks Beyond Downloaded Range**: Readers block on the `Condvar` while the background downloader catches up. On broadband connections, 5 MB audio tracks download in 200–800 ms, rendering catch-up pauses brief.

## Goals vs. Non-Goals

* **In-Scope Goals:**
  * Eliminate the 1:04 playback stoppage permanently for audio and video streams.
  * Zero external binary dependencies (no Deno runtime, no `rustypipe-botguard.exe`, no `yt-dlp`).
  * Cross-platform operation: functional on Windows first, and portable to Linux and macOS without code modifications.
  * Full seek support (both forward and backward) within active streams.
  * Automatic cancellation of obsolete downloads when switching tracks.

* **Out-of-Scope Non-Goals:**
  * Permanent offline media library or persistent track archiving (all cached files are ephemeral).
  * Video transcode/remuxing pipelines (relies on browser-supported AVC1/Opus/AAC codecs).
  * Implementation of YouTube Botguard / PO Token JavaScript VM emulation.

## Alternatives Weighed

* **Bundling `rustypipe-botguard` as a Tauri Sidecar**:
  * *Rejected by author*: Adds 30–40 MB of bundled Deno/JSDOM runtime overhead to the desktop installer and introduces external process management complexity.
* **In-Memory RAM Ring Buffer (`VecDeque<u8>`)**:
  * *Rejected*: Dangerous for video streams (100–300 MB) on low-spec hardware, risking out-of-memory panics.
* **Switching to Android/TV Innertube Client**:
  * *Rejected*: Android and TV clients frequently change signature encryption routines, causing deobfuscation extraction failures in `rustypipe`. iOS streams provide direct unencrypted URLs without signature deciphering.

## Key Decisions & Staging for Next Steps

1. **Storage Location**: Ephemeral cache directory at `app_local_data_dir/stream_cache/`.
2. **Read/Write Coordination**: Standard library `File`, `Condvar`, `AtomicU64`, and `Mutex` without adding external async runtimes or actor frameworks.
3. **Cancellation Mechanism**: Atomic cancellation flags to abort upstream network tasks when tracks change.
4. **Staging**:
   * Replace `proxy.rs` with `StreamSession`, `download_worker`, and `CacheRangeReader`.
   * Add startup cleanup in `src-tauri/src/lib.rs`.
   * Verify continuous playback past 1:04 and forward seeking.
