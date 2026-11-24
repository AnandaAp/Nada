# Critic Review Report

**Target:** `docs/designs/continuous_stream_caching_proxy.md` (Lines 1–217)  
**Reviewers:** `egm-design`, `egm-structural`, `egm-style`

## Summary of Findings
* 🔴 **Blocker**: 3
* 🟡 **Major**: 3
* 🟢 **Nit**: 3

---

## Prioritized Findings (Blockers -> Major -> Nits)

### 1. Incomplete Wait-Condition in `CacheRangeReader` Breaks Forward Seeking - Severity: 🔴 Blocker
* **Reviewer Lens:** `egm-design`
* **Affected Scope:** Lines 146–149 (`CacheRangeReader`)
* **Discrepancy Description**:
  The read invariant currently specifies waiting when `pos == downloaded`. When seeking forward past current progress (e.g. `pos = 5_000_000` while `downloaded = 1_000_000`), `pos > downloaded`. Without waiting when `pos > downloaded`, the reader returns `Ok(0)` (premature EOF), terminating the HTTP response.
* **Recommendation**:
  Update the wait invariant to wait whenever `pos >= downloaded`:
  ```rust
  while self.pos >= self.progress.downloaded.load(Ordering::Acquire) {
      if self.progress.is_complete.load(Ordering::Acquire) || self.progress.is_cancelled.load(Ordering::Acquire) {
          return Ok(0);
      }
      if let Some(err) = self.progress.error.lock().unwrap().as_ref() {
          return Err(std::io::Error::new(std::io::ErrorKind::Other, err.clone()));
      }
      let guard = self.progress.lock.lock().unwrap();
      let _ = self.progress.notify.wait_timeout(guard, Duration::from_millis(200));
  }
  ```

### 2. Missing File Cursor Seek Initialization in `CacheRangeReader` - Severity: 🔴 Blocker
* **Reviewer Lens:** `egm-design`
* **Affected Scope:** Lines 138–149, 169–171
* **Discrepancy Description**:
  When a range request begins at `start > 0`, opening a file initializes the file cursor at offset 0. Without seeking to `start` (`file.seek(SeekFrom::Start(self.pos))`), reads will serve bytes from offset 0, corrupting media playback on seek operations.
* **Recommendation**:
  Mandate that `CacheRangeReader` executes `self.file.seek(SeekFrom::Start(self.pos))?` prior to reading into the output buffer.

### 3. Windows File Sharing and Deletion Violations During Eviction - Severity: 🔴 Blocker
* **Reviewer Lens:** `egm-design`
* **Affected Scope:** Lines 175–179
* **Discrepancy Description**:
  On Windows NTFS, unlinking files while reader handles remain open triggers OS Error 32 (`ERROR_SHARING_VIOLATION`).
* **Recommendation**:
  Enforce non-exclusive sharing flags (`FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`) and wrap temp file removal in best-effort error handling.

### 4. Struct and Field Naming Inconsistencies Across Sections - Severity: 🟡 Major
* **Reviewer Lens:** `egm-structural`
* **Affected Scope:** Lines 103–114 vs 226–233
* **Discrepancy Description**:
  `SharedProgress` names the condition variable `notify` in Architecture vs `cond` in the Implementation Plan.
* **Recommendation**:
  Standardize condition variable naming to `notify: Condvar` across all sections.

### 5. Missing LRU Queue & Active Session Tracking in Proxy Architecture - Severity: 🟡 Major
* **Reviewer Lens:** `egm-structural` / `egm-design`
* **Affected Scope:** Lines 70–80, 126–132, 175–179
* **Discrepancy Description**:
  The document requires LRU eviction (max 10 active tracks) and cancelling previous active sessions, but the proxy model only specifies a basic `HashMap`.
* **Recommendation**:
  Add `active_token: Arc<Mutex<Option<String>>>` and a bounded `lru_order: Arc<Mutex<VecDeque<String>>>` to `StreamProxy`.

### 6. Undefined Metadata Resolution Synchronization Contract - Severity: 🟡 Major
* **Reviewer Lens:** `egm-design`
* **Affected Scope:** Lines 167–169
* **Discrepancy Description**:
  `serve` "waits briefly for `total` and `mime`", but lacks an explicit timeout and fallback error code.
* **Recommendation**:
  Define a 3-second wait on `SharedProgress::notify` for metadata readiness, returning `HTTP 502 Bad Gateway` if upstream connection fails or times out.

### 7. Document Status Formally In Review - Severity: 🟢 Nit
* **Reviewer Lens:** `egm-structural`
* **Recommendation**:
  Keep `Status: In Review` during active review stages until all audits pass.

### 8. Stylistic Precision: Eliminate Promotional Language - Severity: 🟢 Nit
* **Reviewer Lens:** `egm-style`
* **Recommendation**:
  Replace ungrounded assertions ("guarantee seamless", "instantly releasing") with objective technical descriptions.

### 9. Standardize Alternatives Rejection Formatting - Severity: 🟢 Nit
* **Reviewer Lens:** `egm-style`
* **Recommendation**:
  Replace informal "Rejected by author: [reason]" with authoritative objective statements.
