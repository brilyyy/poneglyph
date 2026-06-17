#!/usr/bin/env node
// Downloads the prebuilt poneglyph binary for this platform from GitHub
// Releases and drops it next to bin/poneglyph.js. Never fails `npm install`
// outright — a download miss just leaves the shim to report a clear error
// at run time, with `cargo install poneglyph` as the documented fallback.
'use strict';

const fs = require('fs');
const https = require('https');
const path = require('path');
const zlib = require('zlib');

const REPO = 'brilyyy/poneglyph';
const pkg = require('../package.json');

function targetTriple() {
  const { platform, arch } = process;
  if (platform === 'darwin' && arch === 'arm64') return 'aarch64-apple-darwin';
  if (platform === 'darwin' && arch === 'x64') return 'x86_64-apple-darwin';
  if (platform === 'linux' && arch === 'x64') return 'x86_64-unknown-linux-gnu';
  if (platform === 'win32' && arch === 'x64') return 'x86_64-pc-windows-msvc';
  return null;
}

function fetchFollowingRedirects(url, redirectsLeft) {
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { 'User-Agent': 'poneglyph-npm-postinstall' } }, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        res.resume();
        if (redirectsLeft <= 0) {
          reject(new Error('too many redirects'));
          return;
        }
        resolve(fetchFollowingRedirects(res.headers.location, redirectsLeft - 1));
        return;
      }
      if (res.statusCode !== 200) {
        res.resume();
        reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        return;
      }
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => resolve(Buffer.concat(chunks)));
      res.on('error', reject);
    }).on('error', reject);
  });
}

async function main() {
  if (process.env.PONEGLYPH_SKIP_DOWNLOAD) {
    console.log('poneglyph: PONEGLYPH_SKIP_DOWNLOAD set, skipping binary download.');
    return;
  }

  const target = targetTriple();
  if (!target) {
    console.warn(`poneglyph: no prebuilt binary for ${process.platform}/${process.arch}.`);
    console.warn('Install via: cargo install poneglyph');
    return;
  }

  const isWindows = target.includes('windows');
  const ext = isWindows ? '.exe' : '';
  const asset = `poneglyph-${target}${ext}.gz`;
  const url = `https://github.com/${REPO}/releases/download/v${pkg.version}/${asset}`;
  const destDir = path.join(__dirname, '..', 'bin');
  const destPath = path.join(destDir, `poneglyph${ext}`);

  console.log(`poneglyph: downloading ${asset} from GitHub Releases...`);
  try {
    const gz = await fetchFollowingRedirects(url, 5);
    const binary = zlib.gunzipSync(gz);
    fs.mkdirSync(destDir, { recursive: true });
    fs.writeFileSync(destPath, binary);
    if (!isWindows) fs.chmodSync(destPath, 0o755);
    console.log(`poneglyph: installed binary at ${destPath}`);
  } catch (err) {
    console.warn(`poneglyph: could not download prebuilt binary (${err.message}).`);
    console.warn('Install via: cargo install poneglyph');
  }
}

main();
