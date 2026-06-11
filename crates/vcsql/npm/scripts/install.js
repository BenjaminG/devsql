#!/usr/bin/env node

const https = require('https');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const REPO = 'douglance/devsql';
const BIN_NAME = 'vcsql';

function getPlatformTarget() {
  const platform = process.platform;
  const arch = process.arch;

  const targets = {
    'darwin-x64': 'x86_64-apple-darwin',
    'darwin-arm64': 'aarch64-apple-darwin',
    'linux-x64': 'x86_64-unknown-linux-gnu',
    'linux-arm64': 'aarch64-unknown-linux-gnu',
  };

  const key = `${platform}-${arch}`;
  const target = targets[key];

  if (!target) {
    console.error(`Unsupported platform: ${platform}-${arch}`);
    console.error('Supported platforms: darwin-x64, darwin-arm64, linux-x64, linux-arm64');
    process.exit(1);
  }

  return target;
}

function getVersion() {
  // VCSQL_VERSION env var allows pinning to a specific release (used in tests/CI)
  return process.env.VCSQL_VERSION || require('../package.json').version;
}

function getDownloadUrl(version, target) {
  // Release assets are .tar.xz archives containing vcsql-<target>/vcsql
  return `https://github.com/${REPO}/releases/download/v${version}/${BIN_NAME}-${target}.tar.xz`;
}

function downloadFile(url) {
  return new Promise((resolve, reject) => {
    const handleResponse = (response) => {
      if (response.statusCode === 302 || response.statusCode === 301) {
        https.get(response.headers.location, handleResponse).on('error', reject);
        return;
      }

      if (response.statusCode !== 200) {
        reject(new Error(`Failed to download: ${response.statusCode}`));
        return;
      }

      const chunks = [];
      response.on('data', (chunk) => chunks.push(chunk));
      response.on('end', () => resolve(Buffer.concat(chunks)));
      response.on('error', reject);
    };

    https.get(url, handleResponse).on('error', reject);
  });
}

function extractTarXz(buffer, destDir, target) {
  const tmpFile = path.join(destDir, 'tmp.tar.xz');
  fs.writeFileSync(tmpFile, buffer);
  execSync(`tar -xJf "${tmpFile}" -C "${destDir}"`, { stdio: 'inherit' });
  fs.unlinkSync(tmpFile);

  // The archive contains <BIN_NAME>-<target>/<BIN_NAME>
  const extractedDir = path.join(destDir, `${BIN_NAME}-${target}`);
  fs.copyFileSync(path.join(extractedDir, BIN_NAME), path.join(destDir, BIN_NAME));
  fs.rmSync(extractedDir, { recursive: true, force: true });
}

async function install() {
  const target = getPlatformTarget();
  const version = getVersion();
  const url = getDownloadUrl(version, target);
  const binDir = path.join(__dirname, '..', 'bin');
  const binPath = path.join(binDir, BIN_NAME);

  console.log(`Downloading ${BIN_NAME} v${version} for ${target}...`);

  try {
    const buffer = await downloadFile(url);

    if (!fs.existsSync(binDir)) {
      fs.mkdirSync(binDir, { recursive: true });
    }

    extractTarXz(buffer, binDir, target);
    fs.chmodSync(binPath, 0o755);

    console.log(`Successfully installed ${BIN_NAME} to ${binPath}`);
  } catch (error) {
    console.error(`Failed to install ${BIN_NAME}:`, error.message);
    console.error(`\nYou can install manually from: https://github.com/${REPO}/releases`);
    console.error('Or install via cargo: cargo install vcsql');
    process.exit(1);
  }
}

install();
