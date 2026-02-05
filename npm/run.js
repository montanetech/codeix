#!/usr/bin/env node
"use strict";

const { execFileSync } = require("child_process");
const path = require("path");

const ext = process.platform === "win32" ? ".exe" : "";
const bin = path.join(__dirname, "bin", "codeix" + ext);

try {
  execFileSync(bin, process.argv.slice(2), { stdio: "inherit" });
} catch (e) {
  if (e.status != null) {
    process.exitCode = e.status;
  } else {
    console.error(`codeix: binary not found at ${bin}`);
    console.error("Try reinstalling: npm install -g codeix");
    process.exitCode = 1;
  }
}
