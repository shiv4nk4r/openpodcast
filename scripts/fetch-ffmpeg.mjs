#!/usr/bin/env node
/**
 * Download a platform ffmpeg build into src-tauri/binaries/ for Tauri externalBin.
 * Uses the ffmpeg-static package (prebuilt binaries for macOS/Windows/Linux).
 *
 * Output names match Tauri sidecar convention:
 *   ffmpeg-<rust-target-triple>[.exe]
 */
import { createRequire } from "node:module";
import {
  copyFileSync,
  chmodSync,
  existsSync,
  mkdirSync,
  renameSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const destDir = path.join(root, "src-tauri", "binaries");

function rustTriple() {
  if (process.env.TAURI_ENV_TARGET_TRIPLE) {
    return process.env.TAURI_ENV_TARGET_TRIPLE;
  }
  const { platform, arch } = process;
  if (platform === "darwin" && arch === "arm64") return "aarch64-apple-darwin";
  if (platform === "darwin" && arch === "x64") return "x86_64-apple-darwin";
  if (platform === "win32" && arch === "x64") return "x86_64-pc-windows-msvc";
  if (platform === "win32" && arch === "arm64") return "aarch64-pc-windows-msvc";
  if (platform === "linux" && arch === "x64") return "x86_64-unknown-linux-gnu";
  if (platform === "linux" && arch === "arm64") return "aarch64-unknown-linux-gnu";
  throw new Error(
    `Unsupported host for ffmpeg fetch: ${platform}/${arch}. Set TAURI_ENV_TARGET_TRIPLE.`,
  );
}

function ensureFfmpegStatic() {
  const require = createRequire(path.join(root, "package.json"));
  try {
    return require("ffmpeg-static");
  } catch {
    console.log("Installing ffmpeg-static…");
    execFileSync("npm", ["install", "--no-save", "ffmpeg-static"], {
      cwd: root,
      stdio: "inherit",
      shell: process.platform === "win32",
    });
    return createRequire(path.join(root, "package.json"))("ffmpeg-static");
  }
}

const triple = rustTriple();
const isWin = triple.includes("windows");
const destName = isWin ? `ffmpeg-${triple}.exe` : `ffmpeg-${triple}`;
const dest = path.join(destDir, destName);

mkdirSync(destDir, { recursive: true });

if (existsSync(dest) && !process.env.FFMPEG_FORCE_FETCH) {
  console.log(`ffmpeg sidecar already present: ${dest}`);
  process.exit(0);
}

const src = ensureFfmpegStatic();
if (!src || typeof src !== "string" || !existsSync(src)) {
  console.error("ffmpeg-static did not resolve to a binary path:", src);
  process.exit(1);
}

copyFileSync(src, dest);
try {
  chmodSync(dest, 0o755);
} catch {
  /* windows */
}

// Convenience copy without triple for local `cargo run` discovery.
const plain = path.join(destDir, isWin ? "ffmpeg.exe" : "ffmpeg");
try {
  copyFileSync(dest, plain);
  try {
    chmodSync(plain, 0o755);
  } catch {
    /* windows */
  }
} catch {
  /* ignore */
}

console.log(`Bundled ffmpeg → ${dest}`);
