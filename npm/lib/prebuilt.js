"use strict";

const fs = require("fs");
const path = require("path");
const { targets } = require("../prebuilt/manifest.json");

function platformKey() {
  return `${process.platform}-${process.arch}`;
}

function binaryPath(target) {
  return path.join(__dirname, "..", "prebuilt", target.triple, target.binary);
}

function selectedBinary() {
  const key = platformKey();
  const target = targets[key];
  if (!target) {
    const supported = Object.keys(targets).sort().join(", ");
    throw new Error(`unsupported platform ${key}; supported: ${supported}`);
  }
  return binaryPath(target);
}

function makeExecutable(binary) {
  try {
    fs.chmodSync(binary, 0o755);
  } catch (_) {
    // Best effort only. The runtime wrapper reports execution failures.
  }
}

module.exports = { targets, platformKey, binaryPath, selectedBinary, makeExecutable };
