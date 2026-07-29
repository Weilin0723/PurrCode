"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { targetFor } = require("./platform");

const packageRoot = path.join(__dirname, "..");
const metadata = require(path.join(packageRoot, "package.json"));
const checksums = require(path.join(packageRoot, "checksums.json"));
const allowedHosts = new Set([
  "github.com",
  "objects.githubusercontent.com",
  "release-assets.githubusercontent.com"
]);

function download(url, destination, redirects = 0) {
  if (redirects > 5) return Promise.reject(new Error("too many download redirects"));
  if (url.protocol !== "https:" || !allowedHosts.has(url.hostname)) {
    return Promise.reject(new Error(`refusing untrusted download URL: ${url}`));
  }
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { "User-Agent": `purrcode-npm/${metadata.version}` } }, response => {
      if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        response.resume();
        download(new URL(response.headers.location, url), destination, redirects + 1)
          .then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`download returned HTTP ${response.statusCode}`));
        return;
      }
      const output = fs.createWriteStream(destination, { mode: 0o600 });
      response.pipe(output);
      output.on("finish", () => output.close(resolve));
      output.on("error", reject);
    }).on("error", reject);
  });
}

function digest(file) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash("sha256");
    const input = fs.createReadStream(file);
    input.on("data", chunk => hash.update(chunk));
    input.on("end", () => resolve(hash.digest("hex")));
    input.on("error", reject);
  });
}

async function main() {
  if (process.env.PURRCODE_SKIP_DOWNLOAD === "1") return;
  const target = targetFor(process.platform, process.arch);
  const releaseVersion = metadata.purrcodeRelease || metadata.version;
  const extension = process.platform === "win32" ? "zip" : "tar.gz";
  const archiveName = `purrcode-${target}.${extension}`;
  const expected = checksums[archiveName];
  if (!expected) throw new Error(`missing pinned checksum for ${archiveName}`);

  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "purrcode-npm-"));
  try {
    const archive = path.join(temporary, archiveName);
    const url = new URL(
      `https://github.com/Weilin0723/PurrCode/releases/download/v${releaseVersion}/${archiveName}`
    );
    await download(url, archive);
    const actual = await digest(archive);
    if (actual !== expected) throw new Error(`checksum verification failed for ${archiveName}`);

    const unpacked = path.join(temporary, "unpacked");
    fs.mkdirSync(unpacked);
    const extracted = spawnSync("tar", ["-xf", archive, "-C", unpacked], { stdio: "inherit" });
    if (extracted.status !== 0) throw new Error("the system tar utility could not unpack PurrCode");

    const source = path.join(unpacked, `purrcode-${target}`);
    const destination = path.join(packageRoot, "vendor", target);
    fs.rmSync(destination, { recursive: true, force: true });
    fs.mkdirSync(destination, { recursive: true });
    for (const name of ["purrcode", "purrcoded"]) {
      const executable = process.platform === "win32" ? `${name}.exe` : name;
      fs.copyFileSync(path.join(source, executable), path.join(destination, executable));
      if (process.platform !== "win32") fs.chmodSync(path.join(destination, executable), 0o755);
    }
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}

main().catch(error => {
  console.error(`PurrCode installation failed: ${error.message}`);
  process.exit(1);
});
