import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  assemble,
  hostKey,
  mainManifest,
  pack,
  parseArgs,
  platformManifest,
  readPlatforms,
  readVersion,
  verifyBinary,
} from '../scripts/assemble.mjs';

function tempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'oscar-npm-'));
}

function fakeBinaries(dir, keys) {
  for (const platform of readPlatforms()) {
    if (!keys.includes(platform.key)) continue;
    fs.mkdirSync(path.join(dir, platform.key), { recursive: true });
    fs.writeFileSync(path.join(dir, platform.key, platform.binary), 'binary');
  }
}

test('the version is clients/VERSION', () => {
  assert.match(readVersion(), /^\d+\.\d+\.\d+/);
  const dir = tempDir();
  fs.writeFileSync(path.join(dir, 'VERSION'), 'one\n');
  assert.throws(() => readVersion(path.join(dir, 'VERSION')), /does not hold a version/);
});

test('the six platforms', () => {
  const keys = readPlatforms().map((platform) => platform.key);
  assert.deepEqual(keys, ['darwin-arm64', 'darwin-x64', 'linux-x64', 'linux-arm64', 'win32-x64', 'win32-arm64']);
});

test('the manifests', () => {
  const template = {
    name: '@oscaraccountant/cli',
    version: '0.0.0',
    license: 'MIT',
    homepage: 'https://oscaraccountant.com/docs',
    keywords: ['oscar', 'cli'],
    bin: { oscar: 'bin/oscar.js' },
  };
  const platforms = readPlatforms();
  const main = mainManifest(template, platforms, '1.2.3');
  assert.equal(main.version, '1.2.3');
  assert.equal(Object.keys(main.optionalDependencies).length, 6);
  assert.ok(Object.values(main.optionalDependencies).every((version) => version === '1.2.3'));
  assert.throws(() => mainManifest({ ...template, scripts: { postinstall: 'x' } }, platforms, '1.2.3'), /no install script/);
  const linux = platformManifest(template, platforms[2], '1.2.3');
  assert.deepEqual([linux.name, linux.os, linux.cpu], ['@oscaraccountant/cli-linux-x64', ['linux'], ['x64']]);
  assert.deepEqual(linux.files, ['bin/oscar', 'README.md', 'LICENSE']);
  // The registry page of a platform package links to the same places as the main package.
  assert.equal(linux.homepage, 'https://oscaraccountant.com/docs');
  assert.deepEqual(linux.keywords, ['oscar', 'cli']);
});

test('assembles the seven packages', () => {
  const binaries = tempDir();
  const out = tempDir();
  const keys = readPlatforms().map((platform) => platform.key);
  fakeBinaries(binaries, keys);
  const lines = [];
  const dirs = assemble({ binaries, out, verify: false, log: (line) => lines.push(line) });
  assert.equal(dirs.length, 7);
  assert.equal(path.basename(dirs[6]), 'cli');
  const main = JSON.parse(fs.readFileSync(path.join(out, 'packages', 'cli', 'package.json'), 'utf8'));
  assert.equal(main.version, readVersion());
  assert.equal(main.dependencies, undefined);
  assert.equal(main.scripts, undefined);
  assert.deepEqual(main.bin, { oscar: 'bin/oscar.js' });
  assert.ok(fs.existsSync(path.join(out, 'packages', 'cli', 'bin', 'oscar.js')));
  assert.ok(fs.existsSync(path.join(out, 'packages', 'cli', 'LICENSE')));
  const windows = JSON.parse(fs.readFileSync(path.join(out, 'packages', 'cli-win32-arm64', 'package.json'), 'utf8'));
  assert.deepEqual([windows.os, windows.cpu, windows.version], [['win32'], ['arm64'], readVersion()]);
  assert.ok(fs.existsSync(path.join(out, 'packages', 'cli-win32-arm64', 'bin', 'oscar.exe')));
  if (process.platform !== 'win32') {
    const mode = fs.statSync(path.join(out, 'packages', 'cli-linux-x64', 'bin', 'oscar')).mode;
    assert.equal(mode & 0o111, 0o111);
  }
  assert.equal(lines.length, 7);
});

test('a release needs every binary', () => {
  const binaries = tempDir();
  fakeBinaries(binaries, ['linux-x64']);
  assert.throws(() => assemble({ binaries, out: tempDir(), verify: false }), /binary of darwin-arm64 is missing/);
  const dirs = assemble({ binaries, out: tempDir(), verify: false, allowMissing: true });
  assert.equal(dirs.length, 2);
  assert.throws(() => assemble({ binaries: tempDir(), out: tempDir(), verify: false, allowMissing: true }), /no binary/);
});

test('checks the binary of this machine', { skip: process.platform === 'win32' }, () => {
  const binaries = tempDir();
  fakeBinaries(binaries, [hostKey()]);
  const file = path.join(binaries, hostKey(), 'oscar');
  fs.writeFileSync(file, `#!/bin/sh\necho "oscar-cli ${readVersion()}"\n`);
  fs.chmodSync(file, 0o755);
  verifyBinary(file, readVersion());
  assert.throws(() => verifyBinary(file, '9.9.9'), /not "oscar-cli 9.9.9"/);
  const dirs = assemble({ binaries, out: tempDir(), allowMissing: true });
  assert.equal(dirs.length, 2);
});

test('packs the tarballs', () => {
  const binaries = tempDir();
  const out = tempDir();
  fakeBinaries(binaries, ['linux-x64']);
  const dirs = assemble({ binaries, out, verify: false, allowMissing: true });
  const tarballs = pack(dirs, out);
  assert.deepEqual(
    tarballs.map((file) => path.basename(file)),
    [`oscaraccountant-cli-linux-x64-${readVersion()}.tgz`, `oscaraccountant-cli-${readVersion()}.tgz`],
  );
  assert.ok(tarballs.every((file) => fs.existsSync(file)));
});

test('the command line', () => {
  const options = parseArgs(['--binaries', 'b', '--out', 'o', '--allow-missing', '--pack']);
  assert.equal(options.binaries, path.resolve('b'));
  assert.equal(options.out, path.resolve('o'));
  assert.ok(options.allowMissing && options.pack);
  assert.throws(() => parseArgs([]), /usage/);
  assert.throws(() => parseArgs(['--binaries', 'b', '--nope']), /unknown argument/);
});
