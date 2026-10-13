#!/usr/bin/env node
// Assemble the seven npm packages of the CLI from built binaries, with every version set from
// clients/VERSION.
//
//   node clients/npm/scripts/assemble.mjs --binaries DIR [--out DIR] [--allow-missing] [--pack]
//
// DIR holds one directory per platform key, each with the binary:
//   DIR/linux-x64/oscar, DIR/win32-x64/oscar.exe, ...
// The packages go to OUT/packages/<name>, and with --pack their tarballs to OUT/tarballs.
// Without --allow-missing every platform must have its binary (a release). The binary of the
// machine that runs the script must print `oscar-cli <version>`.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const NPM_DIR = path.resolve(HERE, '..');
export const CLIENTS_DIR = path.resolve(NPM_DIR, '..');
const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

/** The version of every client: clients/VERSION. */
export function readVersion(file = path.join(CLIENTS_DIR, 'VERSION')) {
  const version = fs.readFileSync(file, 'utf8').trim();
  if (!SEMVER.test(version)) {
    throw new Error(`${file} does not hold a version: "${version}"`);
  }
  return version;
}

/** The six platforms. */
export function readPlatforms(file = path.join(NPM_DIR, 'platforms.json')) {
  return JSON.parse(fs.readFileSync(file, 'utf8')).platforms;
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

/** The package.json of the main package, from the template, with the version set. */
export function mainManifest(template, platforms, version) {
  const manifest = { ...template, version };
  manifest.optionalDependencies = Object.fromEntries(
    platforms.map((platform) => [`@oscaraccountant/cli-${platform.key}`, version]),
  );
  if (manifest.scripts && Object.keys(manifest.scripts).some((name) => /install/.test(name))) {
    throw new Error('@oscaraccountant/cli must have no install script');
  }
  return manifest;
}

/** The package.json of a platform package. */
export function platformManifest(template, platform, version) {
  return {
    name: `@oscaraccountant/cli-${platform.key}`,
    version,
    description: `The oscar binary for ${platform.label}. Install @oscaraccountant/cli instead.`,
    license: template.license,
    author: template.author,
    repository: template.repository,
    homepage: template.homepage,
    bugs: template.bugs,
    keywords: template.keywords,
    os: [platform.os],
    cpu: [platform.cpu],
    files: [`bin/${platform.binary}`, 'README.md', 'LICENSE'],
    preferUnplugged: true,
  };
}

/** The key of this machine, `${process.platform}-${process.arch}`. */
export function hostKey() {
  return `${process.platform}-${process.arch}`;
}

/** Check that a binary prints `oscar-cli <version>`. */
export function verifyBinary(binary, version) {
  const output = execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim();
  if (output !== `oscar-cli ${version}`) {
    throw new Error(`${binary} prints "${output}", not "oscar-cli ${version}"`);
  }
}

/** Run npm; on Windows through the shell, because npm is a .cmd file there. */
export function npm(args, options = {}) {
  const windows = process.platform === 'win32';
  return execFileSync(windows ? 'npm.cmd' : 'npm', args, {
    encoding: 'utf8',
    shell: windows,
    ...options,
  });
}

/**
 * Write the packages. Returns the directories of the packages written, platform packages first
 * and the main package last (the order of a publish).
 */
export function assemble({
  binaries,
  out,
  allowMissing = false,
  version = readVersion(),
  platforms = readPlatforms(),
  verify = true,
  log = () => {},
}) {
  const template = JSON.parse(fs.readFileSync(path.join(NPM_DIR, 'cli', 'package.json'), 'utf8'));
  const license = fs.readFileSync(path.join(NPM_DIR, 'cli', 'LICENSE'), 'utf8');
  const packagesDir = path.join(out, 'packages');
  fs.rmSync(packagesDir, { recursive: true, force: true });
  const written = [];
  for (const platform of platforms) {
    const source = path.join(binaries, platform.key, platform.binary);
    if (!fs.existsSync(source)) {
      if (allowMissing) {
        log(`skip ${platform.key}: no ${source}`);
        continue;
      }
      throw new Error(`the binary of ${platform.key} is missing: ${source}`);
    }
    if (verify && platform.key === hostKey()) {
      verifyBinary(source, version);
    }
    const dir = path.join(packagesDir, `cli-${platform.key}`);
    fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
    const target = path.join(dir, 'bin', platform.binary);
    fs.copyFileSync(source, target);
    fs.chmodSync(target, 0o755);
    writeJson(path.join(dir, 'package.json'), platformManifest(template, platform, version));
    fs.writeFileSync(
      path.join(dir, 'README.md'),
      `# @oscaraccountant/cli-${platform.key}\n\nThe \`oscar\` binary for ${platform.label}.\n` +
        'Do not install this package directly. Install `@oscaraccountant/cli`, which picks the\n' +
        'package of your platform, or run `npx @oscaraccountant/cli`.\n',
    );
    fs.writeFileSync(path.join(dir, 'LICENSE'), license);
    written.push(dir);
    log(`wrote ${dir}`);
  }
  if (written.length === 0) {
    throw new Error(`no binary in ${binaries}`);
  }
  const main = path.join(packagesDir, 'cli');
  fs.mkdirSync(path.join(main, 'bin'), { recursive: true });
  fs.copyFileSync(path.join(NPM_DIR, 'cli', 'bin', 'oscar.js'), path.join(main, 'bin', 'oscar.js'));
  fs.chmodSync(path.join(main, 'bin', 'oscar.js'), 0o755);
  fs.copyFileSync(path.join(NPM_DIR, 'cli', 'README.md'), path.join(main, 'README.md'));
  fs.writeFileSync(path.join(main, 'LICENSE'), license);
  writeJson(path.join(main, 'package.json'), mainManifest(template, platforms, version));
  written.push(main);
  log(`wrote ${main}`);
  return written;
}

/** `npm pack` each package into OUT/tarballs. Returns the tarball paths, in publish order. */
export function pack(dirs, out) {
  const tarballs = path.join(out, 'tarballs');
  fs.rmSync(tarballs, { recursive: true, force: true });
  fs.mkdirSync(tarballs, { recursive: true });
  return dirs.map((dir) => {
    const result = JSON.parse(npm(['pack', dir, '--json', '--pack-destination', tarballs]));
    return path.join(tarballs, result[0].filename);
  });
}

export function parseArgs(argv) {
  const options = { out: path.join(NPM_DIR, 'dist'), allowMissing: false, pack: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--binaries') options.binaries = argv[++index];
    else if (arg === '--out') options.out = argv[++index];
    else if (arg === '--allow-missing') options.allowMissing = true;
    else if (arg === '--pack') options.pack = true;
    else throw new Error(`unknown argument: ${arg}`);
  }
  if (!options.binaries) {
    throw new Error('usage: assemble.mjs --binaries DIR [--out DIR] [--allow-missing] [--pack]');
  }
  options.binaries = path.resolve(options.binaries);
  options.out = path.resolve(options.out);
  return options;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const options = parseArgs(process.argv.slice(2));
    const dirs = assemble({ ...options, log: (line) => console.error(line) });
    if (options.pack) {
      for (const tarball of pack(dirs, options.out)) {
        console.log(tarball);
      }
    }
  } catch (error) {
    console.error(`assemble: ${error.message}`);
    process.exit(1);
  }
}
