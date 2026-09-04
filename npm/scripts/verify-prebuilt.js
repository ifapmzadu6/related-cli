"use strict";

const fs = require("fs");

const { targets, binaryPath } = require("../lib/prebuilt");

if (process.env.RELATED_NPM_ALLOW_MISSING_PREBUILT === "1") {
  process.exit(0);
}

const missing = [];
for (const target of Object.values(targets)) {
  const binary = binaryPath(target);
  if (!fs.existsSync(binary)) {
    missing.push(`${target.triple}/${target.binary}`);
  }
}

if (missing.length > 0) {
  console.error("related npm package is missing prebuilt binaries:");
  for (const item of missing) {
    console.error(`  - npm/prebuilt/${item}`);
  }
  console.error("");
  console.error("Stage all binaries before npm pack/publish, or set");
  console.error("RELATED_NPM_ALLOW_MISSING_PREBUILT=1 only for local package-shape tests.");
  process.exit(1);
}
