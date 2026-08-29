'use strict';

// HTTPS GET that honours the proxy the machine is actually configured with.
//
// Node's global `fetch` ignores HTTP_PROXY and HTTPS_PROXY. On a proxied
// machine — which is most corporate networks and a great many home ones — it
// does not fall back, it fails with ECONNRESET, and the install dies for a
// reason that has nothing to do with npm. curl and cargo both read those
// variables, so a package that does not is the odd one out.
//
// So: node:https directly, and where a proxy is configured, a CONNECT tunnel
// through it. No dependency, because a downloader that pulls a dependency to
// download things has a problem the first time the registry is the thing being
// proxied.

const http = require('node:http');
const https = require('node:https');
const net = require('node:net');
const tls = require('node:tls');

const MAX_REDIRECTS = 10;

/// Whether NO_PROXY exempts this host. Matching is by suffix, the way curl and
/// every other client reads it: `.example.com` and `example.com` both cover
/// `api.example.com`.
function isExempt(hostname) {
  const raw = process.env.no_proxy || process.env.NO_PROXY || '';
  for (const entry of raw.split(',')) {
    const pattern = entry.trim().toLowerCase();
    if (!pattern) continue;
    if (pattern === '*') return true;
    const bare = pattern.startsWith('.') ? pattern.slice(1) : pattern;
    const host = hostname.toLowerCase();
    if (host === bare || host.endsWith(`.${bare}`)) return true;
  }
  return false;
}

function proxyFor(url) {
  if (isExempt(url.hostname)) return null;
  const raw =
    url.protocol === 'https:'
      ? process.env.https_proxy || process.env.HTTPS_PROXY
      : process.env.http_proxy || process.env.HTTP_PROXY;
  if (!raw) return null;
  try {
    return new URL(raw.includes('://') ? raw : `http://${raw}`);
  } catch {
    return null;
  }
}

function proxyAuth(proxy) {
  if (!proxy.username && !proxy.password) return undefined;
  const pair = `${decodeURIComponent(proxy.username)}:${decodeURIComponent(proxy.password)}`;
  return `Basic ${Buffer.from(pair).toString('base64')}`;
}

/// Open a raw socket to `url`'s host through `proxy`, using CONNECT.
function tunnel(proxy, url) {
  return new Promise((resolve, reject) => {
    const port = url.port || (url.protocol === 'https:' ? 443 : 80);
    const request = http.request({
      host: proxy.hostname,
      port: proxy.port || 80,
      method: 'CONNECT',
      path: `${url.hostname}:${port}`,
      headers: { host: `${url.hostname}:${port}`, ...(proxyAuth(proxy) ? { 'proxy-authorization': proxyAuth(proxy) } : {}) },
      timeout: 30_000,
    });
    request.once('connect', (response, socket) => {
      if (response.statusCode !== 200) {
        socket.destroy();
        reject(new Error(`proxy ${proxy.host} refused CONNECT with HTTP ${response.statusCode}`));
        return;
      }
      resolve(socket);
    });
    request.once('error', (err) => reject(new Error(`proxy ${proxy.host}: ${err.message}`)));
    request.once('timeout', () => {
      request.destroy();
      reject(new Error(`proxy ${proxy.host} timed out`));
    });
    request.end();
  });
}

/// An https.Agent that reaches the origin through a CONNECT tunnel.
///
/// Going through an Agent is not a stylistic choice. Handing `https.request` a
/// ready-made TLS socket — as `options.socket` or `options.createConnection`
/// returning one — leaves the request waiting for a connection event that will
/// never come, and it hangs until the timeout with nothing on the wire. The
/// Agent's `createConnection(options, callback)` is the supported way to
/// supply a socket asynchronously, and it is what every proxy-agent package
/// does underneath.
class TunnelAgent extends https.Agent {
  constructor(proxy, servername) {
    super({ keepAlive: false });
    this.proxy = proxy;
    this.servername = servername;
  }

  createConnection(options, callback) {
    tunnel(this.proxy, this.target)
      .then((socket) => callback(null, tls.connect({ socket, servername: this.servername })))
      .catch(callback);
  }
}

function once(url, headers) {
  const proxy = proxyFor(url);
  const options = {
    method: 'GET',
    host: url.hostname,
    port: url.port || 443,
    path: `${url.pathname}${url.search}`,
    headers: { host: url.hostname, ...headers },
    timeout: 120_000,
  };
  if (proxy) {
    // TLS is negotiated over the open tunnel, so the proxy never sees the
    // plaintext of the request or the binary that comes back.
    const agent = new TunnelAgent(proxy, url.hostname);
    agent.target = url;
    options.agent = agent;
  }
  return new Promise((resolve, reject) => {
    const request = https.request(options, (response) => {
      const chunks = [];
      response.on('data', (chunk) => chunks.push(chunk));
      response.on('end', () =>
        resolve({
          status: response.statusCode,
          location: response.headers.location,
          body: Buffer.concat(chunks),
        })
      );
      response.on('error', reject);
    });
    request.once('error', reject);
    request.once('timeout', () => {
      request.destroy(new Error(`GET ${url.href} timed out`));
    });
    request.end();
  });
}

/// GET a URL, following redirects, and return the body as a Buffer.
async function get(target, headers = {}) {
  let url = new URL(target);
  for (let hop = 0; hop <= MAX_REDIRECTS; hop += 1) {
    const response = await once(url, headers);
    if (response.status >= 300 && response.status < 400 && response.location) {
      url = new URL(response.location, url);
      continue;
    }
    if (response.status !== 200) {
      throw new Error(`GET ${url.href} returned HTTP ${response.status}`);
    }
    return response.body;
  }
  throw new Error(`GET ${target} redirected more than ${MAX_REDIRECTS} times`);
}

module.exports = { get, proxyFor, isExempt };
