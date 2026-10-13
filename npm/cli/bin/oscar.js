#!/usr/bin/env node
// The `oscar` command of @oscaraccountant/cli. It finds the binary of this platform in its optional
// dependency @oscaraccountant/cli-<platform>-<arch>, runs it with the same arguments, and ends
// with the same exit code or signal. No dependency and no postinstall script: the pattern of
// esbuild and Biome.
'use strict';

const childProcess = require('child_process');
const fs = require('fs');

const PACKAGES = {
  'darwin-arm64': '@oscaraccountant/cli-darwin-arm64',
  'darwin-x64': '@oscaraccountant/cli-darwin-x64',
  'linux-x64': '@oscaraccountant/cli-linux-x64',
  'linux-arm64': '@oscaraccountant/cli-linux-arm64',
  'win32-x64': '@oscaraccountant/cli-win32-x64',
  'win32-arm64': '@oscaraccountant/cli-win32-arm64',
};

// The signals the shim passes on to the binary. On Windows the console sends Ctrl+C to every
// process on it, so the shim only keeps itself alive.
const SIGNALS = ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT', 'SIGBREAK'];

function version() {
  try {
    return require('../package.json').version;
  } catch {
    return 'latest';
  }
}

/** The platform package of this machine, or null when there is none. */
function packageName(platform, arch) {
  return PACKAGES[`${platform}-${arch}`] || null;
}

/** The message when the binary cannot be found. */
function missingMessage(platform, arch, name) {
  if (name === null) {
    return (
      `oscar: there is no oscar binary for ${platform}-${arch}.\n` +
      `The supported platforms are: ${Object.keys(PACKAGES).join(', ')}.\n`
    );
  }
  return (
    `oscar: the binary for ${platform}-${arch} is missing: the optional package ${name} is not installed.\n` +
    'This happens when a lockfile was made on another platform (https://github.com/npm/cli/issues/4828),\n' +
    'or when optional dependencies are turned off (--omit=optional, --no-optional).\n' +
    'To fix it, delete node_modules and package-lock.json and install again with optional dependencies,\n' +
    `or run: npx @oscaraccountant/cli@${version()}\n`
  );
}

/**
 * The path of the binary: OSCAR_CLI_BINARY when set (for tests), else the file of the platform
 * package. null when it is not installed.
 */
function binaryPath(env, platform, arch, resolve) {
  if (env.OSCAR_CLI_BINARY) {
    return env.OSCAR_CLI_BINARY;
  }
  const name = packageName(platform, arch);
  if (name === null) {
    return null;
  }
  const file = platform === 'win32' ? 'oscar.exe' : 'oscar';
  try {
    return resolve(`${name}/bin/${file}`);
  } catch {
    return null;
  }
}

/** Some package managers drop the executable bit; put it back when the file allows it. */
function ensureExecutable(file, platform) {
  if (platform === 'win32') {
    return;
  }
  try {
    fs.accessSync(file, fs.constants.X_OK);
  } catch {
    try {
      fs.chmodSync(file, 0o755);
    } catch {
      // The run below reports the real error.
    }
  }
}

/** The exit code of a process that a signal ended, as a shell reports it. */
function signalExitCode(signal) {
  const number = require('os').constants.signals[signal];
  return 128 + (number || 1);
}

function main() {
  const platform = process.platform;
  const arch = process.arch;
  const binary = binaryPath(process.env, platform, arch, require.resolve);
  if (binary === null) {
    process.stderr.write(missingMessage(platform, arch, packageName(platform, arch)));
    process.exit(1);
  }
  ensureExecutable(binary, platform);
  const child = childProcess.spawn(binary, process.argv.slice(2), {
    stdio: 'inherit',
    windowsHide: false,
  });
  const forwarders = new Map();
  for (const signal of SIGNALS) {
    const forward = () => {
      if (platform !== 'win32' && child.exitCode === null) {
        try {
          child.kill(signal);
        } catch {
          // The child has ended already.
        }
      }
    };
    try {
      process.on(signal, forward);
      forwarders.set(signal, forward);
    } catch {
      // A signal this platform does not have.
    }
  }
  child.on('error', (error) => {
    process.stderr.write(`oscar: cannot run ${binary}: ${error.message}\n`);
    process.exit(1);
  });
  child.on('exit', (code, signal) => {
    for (const [name, forward] of forwarders) {
      process.removeListener(name, forward);
    }
    if (signal) {
      // End the same way, so the caller sees the signal.
      setTimeout(() => process.exit(signalExitCode(signal)), 200);
      try {
        process.kill(process.pid, signal);
      } catch {
        process.exit(signalExitCode(signal));
      }
      return;
    }
    process.exit(code === null ? 1 : code);
  });
}

module.exports = { PACKAGES, binaryPath, missingMessage, packageName, signalExitCode, version };

if (require.main === module) {
  main();
}
