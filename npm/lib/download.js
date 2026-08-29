'use strict';

// Fetching the binary this package is a wrapper around.
//
// The binary lives in the GitHub release for the matching tag, not in the npm
// tarball: seven platform builds would be ~100 MB in one package, and npm
// would carry all of them to every machine to use one. So the package is small
// and the binary arrives on install.
//
// Every download is checked against the release's own SHA256SUMS before it is
// unpacked. That is the same promise install.sh and install.ps1 make, and it
// is not optional — this runs `curl`-shaped code on somebody's machine.

const { createHash } = require('node:crypto');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { describe } = require('./platform.js');
const { get: httpGet, proxyFor } = require('./http.js');

const REPO = 'asale-ai/anything-to-skill';
const { version } = require('../package.json');

const root = path.join(__dirname, '..');
// Keyed by version so an upgrade never runs the previous binary.
const vendorDir = path.join(root, 'vendor', version);

function binaryPath() {
  return path.join(vendorDir, describe().binary);
}

function isInstalled() {
  try {
    fs.accessSync(binaryPath(), fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function get(url) {
  return httpGet(url, { 'user-agent': `anything-to-skill-npm/${version}` });
}

// Pull one asset's digest out of a SHA256SUMS file. Separated from the fetch
// so it can be tested without a network: the parsing is the part that can be
// wrong, and a checksum check that silently matches nothing is no check.
function parseDigest(sums, assetName) {
  for (const line of sums.split('\n')) {
    const match = line.trim().match(/^([0-9a-f]{64})\s+\*?(\S+)$/);
    if (match && match[2] === assetName) return match[1];
  }
  return null;
}

// The expected digest for one asset, read from the release's SHA256SUMS.
async function expectedDigest(base, assetName) {
  const sums = (await get(`${base}/SHA256SUMS`)).toString('utf8');
  const digest = parseDigest(sums, assetName);
  if (!digest) {
    throw new Error(`SHA256SUMS for v${version} does not list ${assetName}`);
  }
  return digest;
}

function unpack(archivePath, intoDir) {
  fs.mkdirSync(intoDir, { recursive: true });
  try {
    // bsdtar reads both .tar.gz and .zip, and ships with macOS, most Linux
    // images, and Windows 10 1803 and later.
    execFileSync('tar', ['-xf', archivePath, '-C', intoDir], { stdio: 'pipe' });
    return;
  } catch (err) {
    if (process.platform !== 'win32') {
      throw new Error(`could not unpack ${path.basename(archivePath)}: ${err.message}`);
    }
  }
  // Older Windows without tar.exe.
  execFileSync(
    'powershell',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      `Expand-Archive -LiteralPath '${archivePath}' -DestinationPath '${intoDir}' -Force`,
    ],
    { stdio: 'pipe' }
  );
}

// The release archive holds one directory; the binary is inside it.
function findBinary(dir, name) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      const found = findBinary(full, name);
      if (found) return found;
    } else if (entry.name === name) {
      return full;
    }
  }
  return null;
}

async function install({ quiet = false } = {}) {
  const say = (message) => {
    if (!quiet) process.stderr.write(`${message}\n`);
  };
  const { triple, archive, binary } = describe();
  const assetName = `anything-to-skill-${version}-${triple}.${archive}`;
  const base = `https://github.com/${REPO}/releases/download/v${version}`;

  const proxy = proxyFor(new URL(base));
  say(
    `anything-to-skill ${version} — fetching the ${triple} binary` +
      (proxy ? ` via ${proxy.protocol}//${proxy.host}` : '')
  );

  const [bytes, expected] = await Promise.all([
    get(`${base}/${assetName}`),
    expectedDigest(base, assetName),
  ]);

  const actual = createHash('sha256').update(bytes).digest('hex');
  if (actual !== expected) {
    throw new Error(
      `checksum mismatch for ${assetName}\n  expected ${expected}\n  got      ${actual}\n` +
        'Nothing was installed.'
    );
  }

  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'anything-to-skill-'));
  try {
    const archivePath = path.join(scratch, assetName);
    fs.writeFileSync(archivePath, bytes);
    unpack(archivePath, scratch);

    const found = findBinary(scratch, binary);
    if (!found) {
      throw new Error(`${assetName} did not contain ${binary}`);
    }
    // Replace atomically enough that a half-written binary is never runnable.
    fs.mkdirSync(vendorDir, { recursive: true });
    const destination = binaryPath();
    fs.copyFileSync(found, `${destination}.partial`);
    fs.chmodSync(`${destination}.partial`, 0o755);
    fs.renameSync(`${destination}.partial`, destination);
    say(`  installed ${destination}`);
    return destination;
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}

module.exports = { install, isInstalled, binaryPath, parseDigest, version, REPO };
