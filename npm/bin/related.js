#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const manifest = require("../prebuilt/manifest.json");

function platformKey() {
  return `${process.platform}-${process.arch}`;
}

function selectedBinary() {
  const key = platformKey();
  const target = manifest.targets[key];
  if (!target) {
    const supported = Object.keys(manifest.targets).sort().join(", ");
    throw new Error(`unsupported platform ${key}; supported: ${supported}`);
  }
  return path.join(__dirname, "..", "prebuilt", target.triple, target.binary);
}

function main() {
  let binary;
  try {
    binary = selectedBinary();
  } catch (error) {
    console.error(`related: ${error.message}`);
    process.exit(1);
  }

  if (!fs.existsSync(binary)) {
    console.error(`related: bundled binary is missing for ${platformKey()}: ${binary}`);
    console.error("related: the npm package was published without the required prebuilt binary");
    process.exit(1);
  }

  if (process.platform !== "win32") {
    try {
      fs.chmodSync(binary, 0o755);
    } catch (_) {
      // If chmod fails, spawn will report the real execution error below.
    }
  }

  const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
  if (result.error) {
    console.error(`related: failed to execute bundled binary: ${result.error.message}`);
    process.exit(1);
  }
  if (result.signal) {
    process.kill(process.pid, result.signal);
    return;
  }
  process.exit(result.status == null ? 1 : result.status);
}

main();
