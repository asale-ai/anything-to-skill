#!/usr/bin/env node
'use strict';

// The launcher. Runs the real binary and gets out of the way.
//
// It re-downloads if the binary is missing, because `npm install
// --ignore-scripts` and some CI caches skip postinstall, and `npx` users
// should not have to know that.

const { spawn } = require('node:child_process');
const { install, isInstalled, binaryPath } = require('../lib/download.js');

async function main() {
  if (!isInstalled()) {
    await install();
  }

  const child = spawn(binaryPath(), process.argv.slice(2), { stdio: 'inherit' });

  // Signals are forwarded rather than swallowed: a crawl interrupted with
  // Ctrl-C has to actually stop, and `mcp` mode is terminated by its client.
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    process.on(signal, () => child.kill(signal));
  }

  child.on('error', (err) => {
    process.stderr.write(`anything-to-skill: could not start the binary: ${err.message}\n`);
    process.exit(1);
  });
  // Exiting with the child's own status is what makes `audit --strict`,
  // `eval --min-pass` and `refresh --check` usable in a pipeline.
  child.on('close', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code === null ? 1 : code);
  });
}

main().catch((err) => {
  process.stderr.write(`anything-to-skill: ${err.message}\n`);
  process.exit(1);
});
