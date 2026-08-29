'use strict';

// npm postinstall. Fetches the binary for this machine.
//
// It fails loudly rather than deferring to first run: an install that reports
// success and leaves an unusable command behind is worse than one that stops
// and says why. The launcher can still recover on its own if this never ran —
// `npm install --ignore-scripts` skips it entirely.

const { install, isInstalled } = require('./download.js');

if (isInstalled()) {
  process.exit(0);
}

install().catch((err) => {
  process.stderr.write(`\nanything-to-skill: ${err.message}\n\n`);
  process.stderr.write(
    'The binary could not be fetched. You can install it another way:\n' +
      '  cargo install anything-to-skill\n' +
      '  https://github.com/asale-ai/anything-to-skill/releases\n\n'
  );
  process.exit(1);
});
