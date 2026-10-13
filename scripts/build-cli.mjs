#!/usr/bin/env node
// Build the `oscar` binary for one Rust target and put it where the npm assembly reads it:
// <out>/<platform key>/oscar[.exe] (clients/npm/platforms.json).
//
//   node clients/scripts/build-cli.mjs --target x86_64-unknown-linux-musl [--out DIR]
//
// A Linux target is a static musl build through `cargo zigbuild` (zig and cargo-zigbuild must be
// installed; CARGO_ZIGBUILD_ZIG_PATH may point at zig). Windows arm64 built on another OS is
// `cargo xwin` with clang (cargo-xwin and clang must be installed), linked by rust-lld, because no
// runner has the MSVC arm64 tools. Every other target is `cargo build`, on its own OS. The default
// OUT is clients/npm/dist/binaries.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const CLIENTS = path.resolve(HERE, '..');
const WORKSPACE = path.join(CLIENTS, 'rust');

/** The platform of a Rust target, from clients/npm/platforms.json. */
export function platformOf(target) {
  const { platforms } = JSON.parse(fs.readFileSync(path.join(CLIENTS, 'npm', 'platforms.json'), 'utf8'));
  const platform = platforms.find((item) => item.target === target);
  if (!platform) {
    const known = platforms.map((item) => item.target).join(', ');
    throw new Error(`unknown target ${target}; the targets are: ${known}`);
  }
  return platform;
}

/** The cargo command line of a target, built on `host` (a `process.platform` value). */
export function cargoArgs(target, host = process.platform) {
  const common = ['--release', '--locked', '-p', 'oscaraccountant-cli', '--target', target];
  if (target.endsWith('-linux-musl')) {
    return ['zigbuild', ...common];
  }
  if (target === 'aarch64-pc-windows-msvc' && host !== 'win32') {
    return ['xwin', 'build', ...common, '--cross-compiler', 'clang'];
  }
  return ['build', ...common];
}

/** The environment of the cargo command: `cargo xwin` links with rust-lld unless told otherwise. */
export function cargoEnv(args, env = process.env) {
  if (args[0] !== 'xwin' || env.CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER) {
    return env;
  }
  return { ...env, CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER: 'rust-lld' };
}

/**
 * `true` when an ELF file has no program interpreter (PT_INTERP): a static binary that runs on
 * any Linux. `null` when the file is not ELF.
 */
export function isStaticElf(file) {
  const bytes = fs.readFileSync(file);
  if (bytes.length < 64 || bytes.readUInt32BE(0) !== 0x7f454c46) {
    return null;
  }
  const is64 = bytes[4] === 2;
  const little = bytes[5] === 1;
  const u16 = (offset) => (little ? bytes.readUInt16LE(offset) : bytes.readUInt16BE(offset));
  const u32 = (offset) => (little ? bytes.readUInt32LE(offset) : bytes.readUInt32BE(offset));
  const u64 = (offset) => Number(little ? bytes.readBigUInt64LE(offset) : bytes.readBigUInt64BE(offset));
  const phoff = is64 ? u64(32) : u32(28);
  const phentsize = u16(is64 ? 54 : 42);
  const phnum = u16(is64 ? 56 : 44);
  for (let index = 0; index < phnum; index += 1) {
    if (u32(phoff + index * phentsize) === 3) {
      return false;
    }
  }
  return true;
}

function parseArgs(argv) {
  const options = { out: path.join(CLIENTS, 'npm', 'dist', 'binaries') };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--target') options.target = argv[++index];
    else if (arg === '--out') options.out = path.resolve(argv[++index]);
    else throw new Error(`unknown argument: ${arg}`);
  }
  if (!options.target) {
    throw new Error('usage: build-cli.mjs --target TARGET [--out DIR]');
  }
  return options;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const { target, out } = parseArgs(process.argv.slice(2));
    const platform = platformOf(target);
    const args = cargoArgs(target);
    execFileSync('cargo', args, { cwd: WORKSPACE, stdio: 'inherit', env: cargoEnv(args) });
    const built = path.join(WORKSPACE, 'target', target, 'release', platform.binary);
    if (platform.os === 'linux' && isStaticElf(built) !== true) {
      throw new Error(`${built} is not a static binary`);
    }
    const dir = path.join(out, platform.key);
    fs.mkdirSync(dir, { recursive: true });
    const copy = path.join(dir, platform.binary);
    fs.copyFileSync(built, copy);
    fs.chmodSync(copy, 0o755);
    console.log(copy);
  } catch (error) {
    console.error(`build-cli: ${error.message}`);
    process.exit(1);
  }
}
