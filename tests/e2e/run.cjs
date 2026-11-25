"use strict";

const { spawnSync, execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const REPO_ROOT = path.resolve(__dirname, "..", "..");
const TAURI_TARGET = path.join(REPO_ROOT, "src-tauri", "target").toLowerCase();

function snapshotNadaProcesses() {
  if (process.platform !== "win32") return [];
  try {
    const tl = execFileSync("tasklist", ["/FI", "IMAGENAME eq nada.exe", "/FO", "CSV", "/NH"], {
      encoding: "utf8",
    }).trim();
    if (!tl || tl.startsWith("INFO:")) return [];
    const out = execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-Command",
        "try { @(Get-Process nada -ErrorAction Stop | Select-Object Id, Path, StartTime) | ConvertTo-Json -Compress } catch { '[]' }",
      ],
      { encoding: "utf8" }
    ).trim();
    if (!out || out === "[]") return [];
    const parsed = JSON.parse(out);
    const arr = Array.isArray(parsed) ? parsed : [parsed];
    return arr.map((p) => ({
      pid: p.Id,
      path: p.Path || "",
      startTime: p.StartTime ? String(p.StartTime) : "",
    }));
  } catch (_) {
    return [];
  }
}

function runStep(label, cmd, args) {
  console.log(`\n=== [E2E] ${label} ===`);
  const res = spawnSync(cmd, args, {
    cwd: REPO_ROOT,
    stdio: "inherit",
    windowsHide: true,
  });
  const ok = res.status === 0;
  console.log(`=== [E2E] ${label}: ${ok ? "PASS" : `FAIL (exit ${res.status})`} ===`);
  return { label, ok, status: res.status };
}

function main() {
  const preProcs = snapshotNadaProcesses();
  const preMap = new Map(preProcs.map((p) => [`${p.pid}:${p.startTime}`, p]));

  const steps = [
    ["Tier 1 — R3 Policy & Hygiene (t1_r3_policy)", "node", ["--test", "tests/e2e/t1_r3_policy.test.cjs"]],
    ["Tier 1 — Frontend Contracts (t1_frontend_contracts)", "node", ["--test", "tests/e2e/t1_frontend_contracts.test.cjs"]],
    ["Tier 2 — Playback Recovery & Transitions (t2_playback_recovery)", "node", ["--test", "tests/e2e/t2_playback_recovery.test.cjs"]],
    ["Tier 2 — Real Edge Browser DOM/16:9/PiP (t2_browser_dom_pip)", "node", ["--test", "tests/e2e/t2_browser_dom_pip.test.cjs"]],
    ["Tier 3 — Cross-Feature Integration (t3_cross_feature)", "node", ["--test", "tests/e2e/t3_cross_feature.test.cjs"]],
    ["Tier 4 — Real-World User Journeys (t4_user_journeys)", "node", ["--test", "tests/e2e/t4_user_journeys.test.cjs"]],
    [
      "Rust Proxy E2E Tiers 1-4 (e2e_proxy)",
      "cargo",
      ["test", "--manifest-path", "src-tauri/Cargo.toml", "--test", "e2e_proxy"],
    ],
  ];

  const runStartMs = Date.now() - 1000;
  const results = steps.map(([label, cmd, args]) => runStep(label, cmd, args));

  // Sweep any temporary Edge profile directories created during this run
  try {
    const tmpDir = os.tmpdir();
    for (const entry of fs.readdirSync(tmpDir, { withFileTypes: true })) {
      if (entry.isDirectory() && entry.name.startsWith("nada-e2e-edge-")) {
        const full = path.join(tmpDir, entry.name);
        try {
          const st = fs.statSync(full);
          if (st.mtimeMs >= runStartMs) {
            fs.rmSync(full, { recursive: true, force: true });
          }
        } catch (_) {}
      }
    }
  } catch (_) {}

  const postProcs = snapshotNadaProcesses();
  const newProcs = postProcs.filter((p) => !preMap.has(`${p.pid}:${p.startTime}`));
  const repoProcs = postProcs.filter((p) => p.path.toLowerCase().startsWith(TAURI_TARGET));

  let hygieneOk = true;
  if (newProcs.length > 0 || repoProcs.length > 0) {
    hygieneOk = false;
    console.error("\n[HYGIENE FAIL] nada.exe process spawned or running from repo target:");
    for (const p of [...newProcs, ...repoProcs]) {
      console.error(`  PID=${p.pid} Path=${p.path}`);
    }
  } else {
    console.log("\n[HYGIENE PASS] Zero new nada.exe processes and zero target/*/nada.exe launches.");
  }

  console.log("\n=== E2E Summary ===");
  for (const r of results) {
    console.log(`- ${r.label}: ${r.ok ? "PASS" : `FAIL (exit ${r.status})`}`);
  }
  console.log(`- Process Hygiene Guard: ${hygieneOk ? "PASS" : "FAIL"}`);

  const allOk = hygieneOk && results.every((r) => r.ok);
  process.exit(allOk ? 0 : 1);
}

main();
