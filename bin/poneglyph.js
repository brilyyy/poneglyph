#!/usr/bin/env node
// Thin exec shim: forwards argv/stdio to the native binary downloaded by
// scripts/npm-postinstall.js. Kept dependency-free so `npm install` never
// needs to resolve anything beyond Node itself.
'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const exeName = process.platform === 'win32' ? 'poneglyph.exe' : 'poneglyph';
const binPath = path.join(__dirname, exeName);

if (!fs.existsSync(binPath)) {
  console.error('poneglyph: native binary not found at ' + binPath);
  console.error('Reinstall to retry the download: npm install -g poneglyph');
  console.error('Or build from source: cargo install poneglyph');
  process.exit(1);
}

const result = spawnSync(binPath, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  console.error('poneglyph: failed to launch native binary:', result.error.message);
  process.exit(1);
}
process.exit(result.status === null ? 1 : result.status);
