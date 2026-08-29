'use strict';

// Which release archive belongs to the machine this is running on.
//
// The names here are the Rust target triples the release workflow builds, and
// they are the contract between this package and GitHub Releases. A platform
// not listed is a platform with no binary — say so by name rather than
// downloading something that cannot run.

const TARGETS = {
  'darwin arm64': 'aarch64-apple-darwin',
  'darwin x64': 'x86_64-apple-darwin',
  'linux arm64': 'aarch64-unknown-linux-gnu',
  'linux x64': 'x86_64-unknown-linux-gnu',
  // There is no aarch64 Windows build. Windows on ARM runs x64 binaries under
  // emulation, so this is a working answer rather than a wrong one.
  'win32 arm64': 'x86_64-pc-windows-msvc',
  'win32 x64': 'x86_64-pc-windows-msvc',
};

// glibc is the default Linux build; a musl system (Alpine) cannot run it.
function isMusl() {
  if (process.platform !== 'linux') return false;
  try {
    const report = typeof process.report?.getReport === 'function' ? process.report.getReport() : null;
    if (report && report.header && typeof report.header.glibcVersionRuntime === 'string') {
      return false;
    }
    // A report with no glibc version on Linux means the runtime is not glibc.
    if (report) return true;
  } catch {
    // Fall through: an unreadable report is not evidence either way.
  }
  return false;
}

function target() {
  const key = `${process.platform} ${process.arch}`;
  const triple = TARGETS[key];
  if (!triple) {
    const supported = [...new Set(Object.keys(TARGETS))].join(', ');
    throw new Error(
      `anything-to-skill has no prebuilt binary for ${key}.\n` +
        `Supported: ${supported}.\n` +
        `Build it from source instead: cargo install anything-to-skill`
    );
  }
  if (triple.endsWith('-unknown-linux-gnu') && isMusl()) {
    return triple.replace('-gnu', '-musl');
  }
  return triple;
}

function describe() {
  const triple = target();
  return {
    triple,
    archive: triple.includes('windows') ? 'zip' : 'tar.gz',
    binary: process.platform === 'win32' ? 'anything-to-skill.exe' : 'anything-to-skill',
  };
}

module.exports = { TARGETS, describe, target, isMusl };
