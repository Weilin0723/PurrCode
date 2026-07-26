"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const dist = process.argv[2];
if (!dist) throw new Error("usage: write-checksums.js <dist-directory>");
const checksums = {};
for (const name of fs.readdirSync(dist).sort()) {
  if (!/^purrcode-(aarch64|x86_64)-.+\.(tar\.gz|zip)$/.test(name)) continue;
  checksums[name] = crypto.createHash("sha256").update(fs.readFileSync(path.join(dist, name))).digest("hex");
}
if (Object.keys(checksums).length !== 5) {
  throw new Error(`expected 5 platform archives, found ${Object.keys(checksums).length}`);
}
fs.writeFileSync(path.join(__dirname, "..", "checksums.json"), `${JSON.stringify(checksums, null, 2)}\n`);
