"use strict";

const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const REPO_ROOT = path.resolve(__dirname, "..", "..");
const SRC_DIR = path.join(REPO_ROOT, "src");
const TAURI_DIR = path.join(REPO_ROOT, "src-tauri");

function walkFiles(dir, predicate = () => true) {
  const out = [];
  if (!fs.existsSync(dir)) return out;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === "target" || entry.name === "node_modules" || entry.name === ".git") {
      continue;
    }
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...walkFiles(full, predicate));
    } else if (predicate(full)) {
      out.push(full);
    }
  }
  return out;
}

test("T1-F7-01: tauri.conf.json enforces 430px fixed width, minHeight 520, maximizable false", () => {
  const conf = JSON.parse(fs.readFileSync(path.join(TAURI_DIR, "tauri.conf.json"), "utf8"));
  const win = conf.app && conf.app.windows && conf.app.windows[0];
  assert.ok(win, "main window config missing");
  assert.equal(win.width, 430);
  assert.equal(win.minWidth, 430);
  assert.equal(win.maxWidth, 430);
  assert.equal(win.minHeight, 520);
  assert.ok(typeof win.height === "number" && win.height >= 520, `height must be >= 520, got ${win.height}`);
  assert.notEqual(win.resizable, false, "window height must remain resizable");
  assert.equal(win.maximizable, false);
  assert.equal(conf.build.frontendDist, "../src");
  assert.equal(conf.app.withGlobalTauri, true);
});

test("T1-F7-02: src-tauri/src/lib.rs enforces DPI-scaled geometry, non-maximizable, and close-to-tray", () => {
  const libRs = fs.readFileSync(path.join(TAURI_DIR, "src", "lib.rs"), "utf8");
  assert.match(libRs, /set_maximizable\(false\)/);
  assert.match(libRs, /430\.0\s*\*\s*scale/);
  assert.match(libRs, /520\.0\s*\*\s*scale/);
  assert.match(libRs, /CloseRequested/);
  assert.match(libRs, /prevent_close\(\)/);
  assert.match(libRs, /window\.hide\(\)/);
  assert.match(libRs, /--autoplay-policy=no-user-gesture-required/);
});

test("T1-F7-03: capabilities/default.json grants only minimal core/window permissions (zero shell/fs)", () => {
  const caps = JSON.parse(
    fs.readFileSync(path.join(TAURI_DIR, "capabilities", "default.json"), "utf8")
  );
  assert.ok(Array.isArray(caps.permissions));
  for (const perm of caps.permissions) {
    assert.doesNotMatch(perm, /^(shell|fs|process|opener|http):/i, `Forbidden capability: ${perm}`);
  }
  assert.ok(caps.permissions.includes("core:default"));
  assert.ok(caps.permissions.includes("core:event:default"));
  assert.ok(caps.permissions.includes("core:window:allow-close"));
});

test("T1-F7-04: tauri.conf.json has no externalBin/resources, targets nsis, and sets strict CSP", () => {
  const conf = JSON.parse(fs.readFileSync(path.join(TAURI_DIR, "tauri.conf.json"), "utf8"));
  const bundle = conf.bundle || {};
  assert.equal(bundle.externalBin, undefined, "externalBin must not be configured");
  assert.equal(bundle.resources, undefined, "external resources must not be bundled");
  assert.ok(Array.isArray(bundle.targets) && bundle.targets.includes("nsis"));

  const csp = conf.app && conf.app.security && conf.app.security.csp;
  assert.ok(typeof csp === "string" && csp.length > 0);
  assert.match(csp, /media-src[^;]*http:\/\/127\.0\.0\.1:\*/);
  assert.match(csp, /connect-src[^;]*http:\/\/127\.0\.0\.1:\*/);
  assert.match(csp, /object-src\s+'none'/);
});

test("T1-F7-05: zero external sidecar binaries exist in repository", () => {
  assert.equal(fs.existsSync(path.join(TAURI_DIR, "binaries")), false, "src-tauri/binaries must not exist");
  const suspicious = /(yt-dlp|ffmpeg|ffprobe|deno|node|botguard|python).*\.(exe|dll|bin)$/i;
  const matches = [
    ...walkFiles(TAURI_DIR, (p) => suspicious.test(path.basename(p))),
    ...walkFiles(SRC_DIR, (p) => /\.(exe|dll|so|dylib)$/i.test(path.basename(p))),
  ];
  assert.deepEqual(matches, []);
});

test("T1-F7-06: Cargo.toml uses rustls-tls and Rust sources never spawn external binaries", () => {
  const cargoToml = fs.readFileSync(path.join(TAURI_DIR, "Cargo.toml"), "utf8");
  assert.match(cargoToml, /reqwest\s*=\s*\{[^}]*default-features\s*=\s*false[^}]*rustls-tls/);
  assert.doesNotMatch(cargoToml, /native-tls/);

  const rsFiles = walkFiles(path.join(TAURI_DIR, "src"), (p) => p.endsWith(".rs"));
  assert.ok(rsFiles.length >= 3);
  for (const file of rsFiles) {
    const src = fs.readFileSync(file, "utf8");
    assert.doesNotMatch(src, /Command::new/, `External process spawn forbidden in ${file}`);
    assert.doesNotMatch(src, /\b(yt-dlp|ffmpeg|deno|rustypipe-botguard)\b/i, `Sidecar ref in ${file}`);
  }
});

test("T1-F7-07: package.json has zero runtime dependencies and preserves baseline test script", () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(REPO_ROOT, "package.json"), "utf8"));
  assert.equal(pkg.dependencies, undefined, "package.json must have zero runtime dependencies");
  assert.equal(pkg.scripts && pkg.scripts.test, "node app.test.cjs");
  assert.deepEqual(Object.keys(pkg.devDependencies || {}), ["@tauri-apps/cli"]);
});

test("T1-F7-08: src/ is pure vanilla HTML/CSS/JS with zero framework or bundler dependencies", () => {
  const srcFiles = walkFiles(SRC_DIR);
  const exts = new Set(srcFiles.map((f) => path.extname(f)));
  for (const ext of exts) {
    assert.ok([".html", ".css", ".js", ".svg", ".png"].includes(ext), `Unexpected asset ext ${ext}`);
  }
  const appJs = fs.readFileSync(path.join(SRC_DIR, "app.js"), "utf8");
  assert.doesNotMatch(appJs, /\brequire\s*\(/);
  assert.doesNotMatch(appJs, /^\s*import\s+.+\s+from\s+['"][^./]/m);
  const indexHtml = fs.readFileSync(path.join(SRC_DIR, "index.html"), "utf8");
  assert.doesNotMatch(indexHtml, /type=["']module["']|vite|webpack|react|vue/i);
});

test("T1-F7-09: E2E test sources never launch or spawn nada.exe", () => {
  const e2eFiles = walkFiles(path.join(REPO_ROOT, "tests", "e2e"), (p) => /\.(c?js|mjs|rs)$/.test(p));
  const rustTestPath = path.join(TAURI_DIR, "tests", "e2e_proxy.rs");
  if (fs.existsSync(rustTestPath)) e2eFiles.push(rustTestPath);

  const spawnNada = /(spawn|execFile|exec|Command::new)\s*\([^)]*nada(\.exe)?/i;
  for (const file of e2eFiles) {
    if (path.basename(file) === "t1_r3_policy.test.cjs" || path.basename(file) === "run.cjs") {
      // Policy & runner scripts inspect process lists / strings without spawning nada.exe
      const src = fs.readFileSync(file, "utf8");
      assert.doesNotMatch(src, /(spawn|execFile|execSync|Command::new)\s*\(\s*['"][^'"]*nada\.exe/i);
      continue;
    }
    const src = fs.readFileSync(file, "utf8");
    assert.doesNotMatch(src, spawnNada, `Forbidden nada.exe launch in ${file}`);
  }
});

test("T1-F7-10: zero repo target/*/nada.exe processes are running", () => {
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
  const repoTargetPrefix = path.join(TAURI_DIR, "target").toLowerCase();
  for (const proc of list) {
    const procPath = (proc.Path || "").toLowerCase();
    assert.equal(
      procPath.startsWith(repoTargetPrefix),
      false,
      `Repo target nada.exe must not be running: PID ${proc.Id} (${proc.Path})`
    );
  }
});
