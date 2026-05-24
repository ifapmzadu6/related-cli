"use strict";

const fs = require("fs");
const path = require("path");

const manifest = require("../prebuilt/manifest.json");

for (const target of Object.values(manifest.targets)) {
  if (target.binary.endsWith(".exe")) {
    continue;
  }
  const binary = path.join(__dirname, "..", "prebuilt", target.triple, target.binary);
  if (!fs.existsSync(binary)) {
    continue;
  }
  try {
    fs.chmodSync(binary, 0o755);
  } catch (_) {
    // Best effort only. The runtime wrapper reports execution failures.
  }
}
