'use strict';

const assert = require('node:assert');
const { test } = require('node:test');

const { proxyFor } = require('../lib/http.js');
const { parseDigest } = require('../lib/download.js');

const KEYS = ['HTTP_PROXY', 'HTTPS_PROXY', 'NO_PROXY', 'http_proxy', 'https_proxy', 'no_proxy'];

function withEnv(env, body) {
  const saved = Object.fromEntries(KEYS.map((k) => [k, process.env[k]]));
  for (const key of KEYS) delete process.env[key];
  Object.assign(process.env, env);
  try {
    return body();
  } finally {
    for (const key of KEYS) delete process.env[key];
    for (const [key, value] of Object.entries(saved)) {
      if (value !== undefined) process.env[key] = value;
    }
  }
}

const GITHUB = new URL('https://github.com/asale-ai/anything-to-skill');

test('no proxy configured means no proxy used', () => {
  assert.equal(withEnv({}, () => proxyFor(GITHUB)), null);
});

test('HTTPS_PROXY is used for https, HTTP_PROXY is not', () => {
  assert.equal(withEnv({ HTTPS_PROXY: 'http://127.0.0.1:7890' }, () => proxyFor(GITHUB)).host, '127.0.0.1:7890');
  assert.equal(withEnv({ HTTP_PROXY: 'http://127.0.0.1:7890' }, () => proxyFor(GITHUB)), null);
});

test('a proxy given without a scheme still parses', () => {
  const proxy = withEnv({ HTTPS_PROXY: '127.0.0.1:7890' }, () => proxyFor(GITHUB));
  assert.equal(proxy.protocol, 'http:');
  assert.equal(proxy.host, '127.0.0.1:7890');
});

test('NO_PROXY exempts the host and its subdomains, and nothing else', () => {
  const env = (no_proxy) => ({ HTTPS_PROXY: 'http://p:1', NO_PROXY: no_proxy });
  assert.equal(withEnv(env('github.com'), () => proxyFor(GITHUB)), null);
  assert.equal(
    withEnv(env('.github.com'), () => proxyFor(new URL('https://api.github.com/x'))),
    null
  );
  assert.equal(withEnv(env('*'), () => proxyFor(GITHUB)), null);
  assert.notEqual(withEnv(env('example.com'), () => proxyFor(GITHUB)), null);
  // A suffix that is not a domain boundary must not match.
  assert.notEqual(withEnv(env('hub.com'), () => proxyFor(GITHUB)), null);
});

test('a digest is read for the right asset only', () => {
  const sums = [
    'a'.repeat(64) + '  anything-to-skill-0.3.0-aarch64-apple-darwin.tar.gz',
    'b'.repeat(64) + '  anything-to-skill-0.3.0-x86_64-apple-darwin.tar.gz',
  ].join('\n');
  assert.equal(parseDigest(sums, 'anything-to-skill-0.3.0-x86_64-apple-darwin.tar.gz'), 'b'.repeat(64));
  // An asset the file does not list must yield nothing, so the caller aborts
  // rather than installing something unverified.
  assert.equal(parseDigest(sums, 'anything-to-skill-0.3.0-x86_64-pc-windows-msvc.zip'), null);
  assert.equal(parseDigest('', 'anything.tar.gz'), null);
});
