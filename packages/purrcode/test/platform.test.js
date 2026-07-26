"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { targetFor } = require("../scripts/platform");

test("maps every published npm platform", () => {
  assert.equal(targetFor("darwin", "arm64"), "aarch64-apple-darwin");
  assert.equal(targetFor("darwin", "x64"), "x86_64-apple-darwin");
  assert.equal(targetFor("linux", "arm64"), "aarch64-unknown-linux-gnu");
  assert.equal(targetFor("linux", "x64"), "x86_64-unknown-linux-gnu");
  assert.equal(targetFor("win32", "x64"), "x86_64-pc-windows-msvc");
  assert.throws(() => targetFor("win32", "arm64"), /does not publish binaries/);
});
