#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const path = require("node:path");

const command = path.basename(process.argv[1]).toLowerCase().startsWith("purrcoded")
  ? "purrcoded"
  : "purrcode";
const executable = process.platform === "win32" ? `${command}.exe` : command;
const target = require("../scripts/platform").targetFor(process.platform, process.arch);
const binary = path.join(__dirname, "..", "vendor", target, executable);
const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(`Unable to start ${command}: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  console.error(`${command} exited after signal ${result.signal}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
