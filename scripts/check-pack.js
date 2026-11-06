'use strict';
// prepack: refuse to pack or publish without both Linux binaries, or with a
// version that differs between package.json and the Rust workspace.
const fs = require('node:fs');
const path = require('node:path');

const root = path.join(__dirname, '..');
const pkg = require(path.join(root, 'package.json'));
const fail = (m) => {
  console.error(`prepack: ${m}`);
  process.exit(1);
};

for (const d of ['linux-x64-gnu', 'linux-x64-musl']) {
  const f = path.join(root, 'bin', d, 's7napi.node');
  if (!fs.existsSync(f)) fail(`missing ${path.relative(root, f)}; run "npm run build:release"`);
  const elf = fs.readFileSync(f).subarray(0, 4);
  if (elf.toString('latin1') !== '\x7fELF') fail(`${path.relative(root, f)} is not an ELF file`);
}

const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const m = cargo.match(/\[workspace\.package\][^[]*?version\s*=\s*"([^"]+)"/);
if (!m) fail('no version in [workspace.package] of Cargo.toml');
if (m[1] !== pkg.version) fail(`version mismatch: package.json ${pkg.version}, Cargo.toml ${m[1]}`);
console.log(`prepack: ok (${pkg.name}@${pkg.version}, gnu + musl binaries)`);
