'use strict';

const assert = require('node:assert/strict');
const childProcess = require('node:child_process');
const path = require('node:path');
const test = require('node:test');

const shim = require('../cli/bin/oscar.js');

const SHIM = path.join(__dirname, '..', 'cli', 'bin', 'oscar.js');

function runShim(args, env) {
  return childProcess.spawnSync(process.execPath, [SHIM, ...args], {
    encoding: 'utf8',
    env: { ...process.env, ...env },
  });
}

test('one package per supported platform', () => {
  assert.equal(shim.packageName('linux', 'x64'), '@oscaraccountant/cli-linux-x64');
  assert.equal(shim.packageName('win32', 'arm64'), '@oscaraccountant/cli-win32-arm64');
  assert.equal(shim.packageName('darwin', 'arm64'), '@oscaraccountant/cli-darwin-arm64');
  assert.equal(shim.packageName('freebsd', 'x64'), null);
  assert.equal(shim.packageName('linux', 'ia32'), null);
  assert.equal(Object.keys(shim.PACKAGES).length, 6);
});

test('finds the binary of the platform package', () => {
  const resolved = [];
  const resolve = (name) => {
    resolved.push(name);
    return `/modules/${name}`;
  };
  assert.equal(shim.binaryPath({}, 'linux', 'x64', resolve), '/modules/@oscaraccountant/cli-linux-x64/bin/oscar');
  assert.equal(
    shim.binaryPath({}, 'win32', 'x64', resolve),
    '/modules/@oscaraccountant/cli-win32-x64/bin/oscar.exe',
  );
  assert.equal(shim.binaryPath({ OSCAR_CLI_BINARY: '/x/oscar' }, 'linux', 'x64', resolve), '/x/oscar');
  const missing = () => {
    throw new Error('Cannot find module');
  };
  assert.equal(shim.binaryPath({}, 'linux', 'x64', missing), null);
  assert.equal(shim.binaryPath({}, 'sunos', 'x64', resolve), null);
});

test('says what to do when the binary is missing', () => {
  const message = shim.missingMessage('linux', 'x64', '@oscaraccountant/cli-linux-x64');
  assert.match(message, /@oscaraccountant\/cli-linux-x64 is not installed/);
  assert.match(message, /npm\/cli\/issues\/4828/);
  assert.match(message, /npx @oscaraccountant\/cli@0\.1\.0/);
  assert.match(shim.missingMessage('sunos', 'x64', null), /no oscar binary for sunos-x64/);
  assert.equal(shim.version(), require('../cli/package.json').version);
  const run = runShim(['--version'], { OSCAR_CLI_BINARY: '' });
  assert.equal(run.status, 1);
  assert.match(run.stderr, /is missing|no oscar binary/);
});

test('passes the arguments, the output and the exit code', () => {
  const run = runShim(['-e', 'console.log(process.argv.slice(1).join(",")); process.exit(7)', 'a b', '--c'], {
    OSCAR_CLI_BINARY: process.execPath,
  });
  assert.equal(run.stdout.trim(), 'a b,--c');
  assert.equal(run.status, 7);
});

test('reports a binary that cannot run', () => {
  const run = runShim([], { OSCAR_CLI_BINARY: path.join(__dirname, 'no-such-binary') });
  assert.equal(run.status, 1);
  assert.match(run.stderr, /cannot run/);
});

test('the exit code of a signal', () => {
  assert.equal(shim.signalExitCode('SIGTERM'), 143);
  assert.equal(shim.signalExitCode('SIGINT'), 130);
  assert.equal(shim.signalExitCode('SIGNOPE'), 129);
});

test('passes a signal on and ends with it', { skip: process.platform === 'win32' }, async () => {
  const child = childProcess.spawn(
    process.execPath,
    [SHIM, '-e', 'process.on("SIGTERM", () => { console.log("got"); process.exit(3); }); setInterval(() => {}, 1000); console.log("ready");'],
    { env: { ...process.env, OSCAR_CLI_BINARY: process.execPath } },
  );
  let output = '';
  await new Promise((resolve) => {
    child.stdout.on('data', (chunk) => {
      output += chunk;
      if (output.includes('ready')) resolve();
    });
  });
  child.kill('SIGTERM');
  const [code] = await new Promise((resolve) => child.on('exit', (...result) => resolve(result)));
  assert.match(output, /got/);
  assert.equal(code, 3);

  const killed = childProcess.spawn(
    process.execPath,
    [SHIM, '-e', 'setInterval(() => {}, 1000); console.log("ready");'],
    { env: { ...process.env, OSCAR_CLI_BINARY: process.execPath } },
  );
  await new Promise((resolve) => killed.stdout.on('data', resolve));
  killed.kill('SIGHUP');
  const [, signal] = await new Promise((resolve) => killed.on('exit', (...result) => resolve(result)));
  assert.equal(signal, 'SIGHUP');
});
