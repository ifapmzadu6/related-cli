"use strict";

const fs = require("fs");

const { targets, binaryPath, makeExecutable } = require("../lib/prebuilt");

for (const target of Object.values(targets)) {
  if (target.binary.endsWith(".exe")) {
    continue;
  }
  const binary = binaryPath(target);
  if (!fs.existsSync(binary)) {
    continue;
  }
  makeExecutable(binary);
}
