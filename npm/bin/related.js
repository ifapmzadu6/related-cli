#!/usr/bin/env node
"use strict";

const fs = require("fs");
const { spawnSync } = require("child_process");

const { platformKey, selectedBinary, makeExecutable } = require("../lib/prebuilt");

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
    makeExecutable(binary);
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
