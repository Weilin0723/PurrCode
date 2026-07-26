"use strict";

function targetFor(platform, architecture) {
  const targets = {
    "darwin-arm64": "aarch64-apple-darwin",
    "darwin-x64": "x86_64-apple-darwin",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "linux-x64": "x86_64-unknown-linux-gnu",
    "win32-x64": "x86_64-pc-windows-msvc"
  };
  const target = targets[`${platform}-${architecture}`];
  if (!target) {
    throw new Error(`PurrCode does not publish binaries for ${platform}/${architecture}`);
  }
  return target;
}

module.exports = { targetFor };
