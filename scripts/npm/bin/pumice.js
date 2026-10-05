#!/usr/bin/env node
'use strict';

// Launcher for the pumice npm package. It locates the platform-specific
// native package (@bresillero/pumice-<os>-<arch>, an optionalDependency of
// this package), verifies it, and runs the real executable with the
// caller's argv/stdin/stdout/stderr. No network access, no downloads, no
// shell, zero dependencies. Requires Node.js >= 22.

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

function fail(message) {
  console.error(`pumice: ${message}`);
  process.exit(1);
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (err) {
    fail(`cannot read ${file}: ${err.message}`);
  }
}

const frontPkgPath = path.join(__dirname, '..', 'package.json');
const frontPkg = readJson(frontPkgPath);
const platformKey = `${process.platform}-${process.arch}`;

// The native package for this platform is the optionalDependency whose name
// ends with the platform key (for example @bresillero/pumice-linux-x64).
const optional = frontPkg.optionalDependencies || {};
const nativeName = Object.keys(optional).find((name) =>
  name.endsWith(`-${platformKey}`)
);
if (!nativeName) {
  const base = `${frontPkg.name}-`;
  const supported = Object.keys(optional)
    .map((name) => name.slice(name.lastIndexOf('/') + 1))
    .filter((baseName) => baseName.startsWith(base))
    .map((baseName) => baseName.slice(base.length))
    .join(', ');
  fail(
    `unsupported platform ${platformKey} (supported: ${supported || 'none listed'}).\n` +
      'pumice ships prebuilt binaries for the platforms above; it never downloads one.\n' +
      'Use a release archive from the GitHub releases page on other platforms.'
  );
}

let nativePkgPath;
try {
  // Resolution starts at this launcher's own directory so it works under
  // pnpm's isolated node_modules layout, where this package sees its
  // dependencies through symlinks. process.cwd() is deliberately NOT a
  // fallback: a random calling directory must never satisfy the dependency.
  nativePkgPath = require.resolve(`${nativeName}/package.json`, {
    paths: [__dirname],
  });
} catch {
  fail(
    `missing native package ${nativeName}.\n` +
      'Reinstall pumice so the optional dependency is present, for example:\n' +
      '  npm install -g pumice'
  );
}

const nativePkg = readJson(nativePkgPath);
if (nativePkg.version !== frontPkg.version) {
  fail(
    `version mismatch: ${nativeName} is ${nativePkg.version} but pumice is ${frontPkg.version}.\n` +
      'Reinstall pumice so both packages are the same version.'
  );
}

const executable = process.platform === 'win32' ? 'pumice.exe' : 'pumice';
const binary = path.join(path.dirname(nativePkgPath), 'bin', executable);
try {
  if (!fs.statSync(binary).isFile()) throw new Error('not a file');
} catch {
  fail(`native binary missing: ${binary}.\nReinstall pumice to restore it.`);
}

const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });

child.on('error', (err) => {
  fail(`failed to start ${binary}: ${err.message}`);
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    try {
      child.kill(signal);
    } catch {
      // The child already exited; the exit handler finishes the process.
    }
  });
}

child.on('exit', (code, signal) => {
  if (signal) {
    // Die by the same signal so shells and callers observe the same status.
    process.removeAllListeners(signal);
    try {
      process.kill(process.pid, signal);
    } catch {
      // Fall through to a plain error exit.
    }
    process.exit(1);
  }
  process.exit(code === null ? 1 : code);
});
