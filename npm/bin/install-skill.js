#!/usr/bin/env node
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");

function usage() {
  console.log(`Usage:
  related-install-skill [codex] [--user]
  related-install-skill claude [--user]

Installs or updates the find-related-files skill by copying this package's
skills/find-related-files directory into the selected agent skill directory.
With no arguments, installs the Codex project skill into the current working
directory. Use --user for a user-level install.`);
}

function removePath(target) {
  if (!fs.existsSync(target)) {
    return;
  }
  if (fs.rmSync) {
    fs.rmSync(target, { force: true, recursive: true });
    return;
  }
  if (!fs.existsSync(target)) {
    return;
  }
  const stat = fs.lstatSync(target);
  if (!stat.isDirectory() || stat.isSymbolicLink()) {
    fs.unlinkSync(target);
    return;
  }
  for (const entry of fs.readdirSync(target)) {
    removePath(path.join(target, entry));
  }
  fs.rmdirSync(target);
}

function copyDir(source, dest) {
  fs.mkdirSync(dest, { recursive: true });
  for (const entry of fs.readdirSync(source, { withFileTypes: true })) {
    const sourcePath = path.join(source, entry.name);
    const destPath = path.join(dest, entry.name);
    if (entry.isDirectory()) {
      copyDir(sourcePath, destPath);
    } else if (entry.isSymbolicLink()) {
      fs.symlinkSync(fs.readlinkSync(sourcePath), destPath);
    } else {
      fs.copyFileSync(sourcePath, destPath);
      fs.chmodSync(destPath, fs.statSync(sourcePath).mode);
    }
  }
}

function pinRuntimeVersion(skillDir, packageVersion) {
  const skillPath = path.join(skillDir, "SKILL.md");
  const source = fs.readFileSync(skillPath, "utf8");
  const pinned = source.replace(
    /related-cli@latest(?= related (?:audit|query|diff)\b)/g,
    `related-cli@${packageVersion}`,
  );
  if (pinned === source) {
    throw new Error(`missing runtime package reference in ${skillPath}`);
  }
  fs.writeFileSync(skillPath, pinned);
}

function parseArgs(argv) {
  let agent = "codex";
  let scope = "project";
  for (const arg of argv) {
    if (arg === "codex") {
      agent = "codex";
    } else if (arg === "claude") {
      agent = "claude";
    } else if (arg === "--user") {
      scope = "user";
    } else if (arg === "-h" || arg === "--help" || arg === "help") {
      usage();
      process.exit(0);
    } else {
      usage();
      process.exit(2);
    }
  }
  return { agent, scope };
}

function destination({ agent, scope }) {
  if (agent === "codex" && scope === "project") {
    return path.join(process.cwd(), ".agents", "skills", "find-related-files");
  }
  if (agent === "codex" && scope === "user") {
    return path.join(
      os.homedir(),
      ".agents",
      "skills",
      "find-related-files",
    );
  }
  if (agent === "claude" && scope === "project") {
    return path.join(process.cwd(), ".claude", "skills", "find-related-files");
  }
  return path.join(os.homedir(), ".claude", "skills", "find-related-files");
}

function installSkill(skillSource, dest, packageVersion) {
  if (!fs.existsSync(path.join(skillSource, "SKILL.md"))) {
    throw new Error(`missing skill source: ${skillSource}`);
  }

  const destParent = path.dirname(dest);
  const destName = path.basename(dest);
  fs.mkdirSync(destParent, { recursive: true });

  const tmp = fs.mkdtempSync(path.join(destParent, `.${destName}.tmp.`));
  let backup = "";
  let installed = false;
  try {
    copyDir(skillSource, tmp);
    pinRuntimeVersion(tmp, packageVersion);
    if (fs.existsSync(dest)) {
      backup = fs.mkdtempSync(path.join(destParent, `.${destName}.backup.`));
      fs.rmdirSync(backup);
      fs.renameSync(dest, backup);
    }
    fs.renameSync(tmp, dest);
    installed = true;
    if (backup) {
      removePath(backup);
      backup = "";
    }
  } catch (error) {
    if (backup && fs.existsSync(backup) && !fs.existsSync(dest)) {
      fs.renameSync(backup, dest);
      backup = "";
    }
    throw error;
  } finally {
    if (fs.existsSync(tmp)) {
      removePath(tmp);
    }
    if (installed && backup && fs.existsSync(backup)) {
      removePath(backup);
    }
  }
}

function main() {
  const packageRoot = path.resolve(__dirname, "..", "..");
  const packageVersion = require(path.join(packageRoot, "package.json")).version;
  const skillSource = path.join(packageRoot, "skills", "find-related-files");
  const dest = destination(parseArgs(process.argv.slice(2)));
  installSkill(skillSource, dest, packageVersion);
  console.log(`installed find-related-files skill to ${dest}`);
}

try {
  main();
} catch (error) {
  console.error(`related-install-skill: ${error.message}`);
  process.exit(1);
}
