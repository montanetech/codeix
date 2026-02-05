#!/usr/bin/env node
"use strict";

// Postinstall script: downloads the prebuilt codeix binary from GitHub Releases.
// Zero dependencies — uses Node.js built-in modules only.

const https = require("https");
const fs = require("fs");
const path = require("path");
const { execSync } = require("child_process");
const zlib = require("zlib");

const VERSION = require("./package.json").version;
const REPO = "montanetech/codeix";

const PLATFORM_MAP = {
  "darwin-arm64": "aarch64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-msvc",
};

function getTarget() {
  const key = `${process.platform}-${process.arch}`;
  const target = PLATFORM_MAP[key];
  if (!target) {
    console.error(`codeix: unsupported platform ${key}`);
    console.error(`Supported: ${Object.keys(PLATFORM_MAP).join(", ")}`);
    process.exit(1);
  }
  return target;
}

function getArchiveUrl(target) {
  const ext = process.platform === "win32" ? "zip" : "tar.gz";
  return `https://github.com/${REPO}/releases/download/v${VERSION}/codeix-${target}.${ext}`;
}

function download(url) {
  return new Promise((resolve, reject) => {
    https.get(url, (res) => {
      // Follow redirects (GitHub releases redirect to S3)
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        return download(res.headers.location).then(resolve, reject);
      }
      if (res.statusCode !== 200) {
        return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
      }
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => resolve(Buffer.concat(chunks)));
      res.on("error", reject);
    }).on("error", reject);
  });
}

function extractTarGz(buffer, destDir) {
  // Write to temp file then extract with tar (available on macOS/Linux)
  const tmpFile = path.join(destDir, "_codeix.tar.gz");
  fs.writeFileSync(tmpFile, buffer);
  execSync(`tar xzf "${tmpFile}" -C "${destDir}"`, { stdio: "ignore" });
  fs.unlinkSync(tmpFile);
}

function extractZip(buffer, destDir) {
  // Write to temp file then extract with PowerShell (Windows)
  const tmpFile = path.join(destDir, "_codeix.zip");
  fs.writeFileSync(tmpFile, buffer);
  execSync(
    `powershell -Command "Expand-Archive -Path '${tmpFile}' -DestinationPath '${destDir}' -Force"`,
    { stdio: "ignore" }
  );
  fs.unlinkSync(tmpFile);
}

async function main() {
  const target = getTarget();
  const url = getArchiveUrl(target);
  const binDir = path.join(__dirname, "bin");

  // Skip if binary already exists (e.g. CI caching)
  const binName = process.platform === "win32" ? "codeix.exe" : "codeix";
  const binPath = path.join(binDir, binName);
  if (fs.existsSync(binPath)) {
    return;
  }

  console.log(`Downloading codeix v${VERSION} for ${target}...`);

  fs.mkdirSync(binDir, { recursive: true });

  const buffer = await download(url);

  if (process.platform === "win32") {
    extractZip(buffer, binDir);
  } else {
    extractTarGz(buffer, binDir);
    fs.chmodSync(binPath, 0o755);
  }

  console.log(`Installed codeix to ${binPath}`);
}

main().catch((err) => {
  console.error(`codeix install failed: ${err.message}`);
  console.error("You can install manually from:");
  console.error(`  https://github.com/${REPO}/releases/tag/v${VERSION}`);
  // Don't fail the install — the binary just won't be available
  // This avoids breaking `npm install` in CI environments that don't need the binary
  process.exit(0);
});
