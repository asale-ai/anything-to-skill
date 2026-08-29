'use strict';

const assert = require('node:assert');
const { test } = require('node:test');

const { TARGETS, target } = require('../lib/platform.js');

function withPlatform(platform, arch, body) {
  const original = { platform: process.platform, arch: process.arch };
  Object.defineProperty(process, 'platform', { value: platform, configurable: true });
  Object.defineProperty(process, 'arch', { value: arch, configurable: true });
  try {
    return body();
  } finally {
    Object.defineProperty(process, 'platform', { ...original, value: original.platform, configurable: true });
    Object.defineProperty(process, 'arch', { value: original.arch, configurable: true });
  }
}

test('every supported platform maps to a target the release builds', () => {
  // These are the exact triples .github/workflows/release.yml produces. A name
  // that drifts here downloads a 404.
  const built = new Set([
    'aarch64-apple-darwin',
    'x86_64-apple-darwin',
    'x86_64-unknown-linux-gnu',
    'aarch64-unknown-linux-gnu',
    'x86_64-unknown-linux-musl',
    'aarch64-unknown-linux-musl',
    'x86_64-pc-windows-msvc',
  ]);
  for (const triple of Object.values(TARGETS)) {
    assert.ok(built.has(triple), `${triple} is not a target the release builds`);
  }
});

test('the common platforms resolve', () => {
  assert.equal(withPlatform('darwin', 'arm64', target), 'aarch64-apple-darwin');
  assert.equal(withPlatform('darwin', 'x64', target), 'x86_64-apple-darwin');
  assert.equal(withPlatform('win32', 'x64', target), 'x86_64-pc-windows-msvc');
});

test('windows on arm falls back to the x64 build rather than failing', () => {
  // There is no aarch64 Windows build, and Windows on ARM emulates x64.
  assert.equal(withPlatform('win32', 'arm64', target), 'x86_64-pc-windows-msvc');
});

test('an unsupported platform is named, not guessed at', () => {
  assert.throws(
    () => withPlatform('sunos', 'x64', target),
    /no prebuilt binary for sunos x64[\s\S]*cargo install/
  );
});
