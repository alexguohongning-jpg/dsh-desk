#!/usr/bin/env node
/**
 * prepare-runtime.mjs —— 组装自包含运行时（构建/开发前执行，幂等）：
 *
 *   src-tauri/binaries/node-<triple>.exe   官方 Node.js（sidecar，必须带 target-triple 后缀）
 *   src-tauri/resources/dsh-runtime/       @deepseek-ai/dsh 固定版本整树（npm install 产物）
 *                                          + pnpm.exe（dsh plugin 命令转发给 pnpm，必须内置）
 *
 * 版本全部固定（ref 安装偏好），升级 = 改下面的常量后重新构建发版。
 */
import { execFileSync } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, rmSync, cpSync, writeFileSync } from "node:fs";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import os from "node:os";

// ── 固定版本（升级时只改这里）───────────────────────────────────────────
const NODE_VERSION = "22.14.0";        // Node 22 LTS
const PNPM_VERSION = "10.9.0";         // pnpm standalone (win-x64)
const DSH_VERSION = "0.1.2-rc.1";      // @deepseek-ai/dsh（当前全局安装的同版本）
// ────────────────────────────────────────────────────────────────────────

const argv = new Set(process.argv.slice(2));
const TRIPLE = argv.has("--triple")
  ? process.argv[process.argv.indexOf("--triple") + 1]
  : "x86_64-pc-windows-msvc";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const binariesDir = path.join(root, "src-tauri", "binaries");
const resourcesDir = path.join(root, "src-tauri", "resources", "dsh-runtime");
const buildTmp = path.join(root, "src-tauri", ".runtime-build");

const nodeExe = path.join(binariesDir, `node-${TRIPLE}.exe`);
const dshEntry = path.join(resourcesDir, "node_modules", "@deepseek-ai", "dsh", "lib", "bin.js");
const pnpmExe = path.join(resourcesDir, "pnpm.exe");

async function download(url, dest) {
  console.log(`[runtime] 下载 ${url}`);
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) throw new Error(`下载失败 HTTP ${res.status}: ${url}`);
  await pipeline(res.body, createWriteStream(dest));
}

if (argv.has("--if-needed") && existsSync(nodeExe) && existsSync(dshEntry) && existsSync(pnpmExe)) {
  console.log("[runtime] 已组装，跳过（--if-needed）");
  process.exit(0);
}

mkdirSync(binariesDir, { recursive: true });
mkdirSync(path.join(root, "src-tauri", "resources"), { recursive: true });

// 1) Node.js（用系统 tar 解压，Windows 10+ 自带 bsdtar 支持 zip）
if (!existsSync(nodeExe)) {
  const tmp = path.join(os.tmpdir(), `dsh-desk-node-${NODE_VERSION}`);
  rmSync(tmp, { recursive: true, force: true });
  mkdirSync(tmp, { recursive: true });
  const zip = path.join(tmp, "node.zip");
  await download(`https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-win-x64.zip`, zip);
  execFileSync("tar", ["-xf", zip, "-C", tmp, "--strip-components=1"], { stdio: "inherit" });
  cpSync(path.join(tmp, "node.exe"), nodeExe);
  rmSync(tmp, { recursive: true, force: true });
  console.log(`[runtime] node.exe → ${nodeExe}`);
}

// 2) dsh 整树（固定版本，生产依赖）
if (!existsSync(dshEntry)) {
  rmSync(buildTmp, { recursive: true, force: true });
  mkdirSync(buildTmp, { recursive: true });
  writeFileSync(path.join(buildTmp, "package.json"), JSON.stringify({ name: "dsh-runtime", private: true }));
  console.log(`[runtime] npm install @deepseek-ai/dsh@${DSH_VERSION} --omit=dev`);
  execFileSync(
    process.platform === "win32" ? "npm.cmd" : "npm",
    ["install", `@deepseek-ai/dsh@${DSH_VERSION}`, "--omit=dev", "--no-audit", "--no-fund", "--ignore-scripts"],
    { cwd: buildTmp, stdio: "inherit" }
  );
  rmSync(resourcesDir, { recursive: true, force: true });
  mkdirSync(resourcesDir, { recursive: true });
  cpSync(path.join(buildTmp, "node_modules"), path.join(resourcesDir, "node_modules"), { recursive: true });
  cpSync(path.join(buildTmp, "package.json"), path.join(resourcesDir, "package.json"));
  rmSync(buildTmp, { recursive: true, force: true });
  console.log(`[runtime] dsh@${DSH_VERSION} → ${resourcesDir}`);
}

// 3) pnpm standalone（dsh plugin 子命令需要）
if (!existsSync(pnpmExe)) {
  await download(`https://github.com/pnpm/pnpm/releases/download/v${PNPM_VERSION}/pnpm-win-x64.exe`, pnpmExe);
  console.log(`[runtime] pnpm@${PNPM_VERSION} → ${pnpmExe}`);
}

console.log("[runtime] 组装完成");
