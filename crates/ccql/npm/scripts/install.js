#!/usr/bin/env node

const { execSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const https = require('https');

// CCQL_VERSION env var allows pinning to a specific release (used in tests/CI)
const VERSION = process.env.CCQL_VERSION || require('../package.json').version;
const REPO = 'douglance/devsql';

const PLATFORM_MAP = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'linux-x64': 'x86_64-unknown-linux-gnu',
};

async function downloadFile(url, dest) {
  return new Promise((resolve, reject) => {
    https.get(url, (response) => {
      if (response.statusCode === 302 || response.statusCode === 301) {
        downloadFile(response.headers.location, dest).then(resolve).catch(reject);
        return;
      }
      if (response.statusCode !== 200) {
        reject(new Error(`Failed to download: ${response.statusCode}`));
        return;
      }
      const file = fs.createWriteStream(dest);
      response.pipe(file);
      file.on('finish', () => {
        file.close();
        resolve();
      });
      file.on('error', reject);
    }).on('error', reject);
  });
}

async function main() {
  const platform = `${process.platform}-${process.arch}`;
  const target = PLATFORM_MAP[platform];

  if (!target) {
    console.error(`Unsupported platform: ${platform}`);
    console.error('Supported: darwin-arm64, darwin-x64, linux-arm64, linux-x64');
    console.error('Please install from source: cargo install ccql');
    process.exit(1);
  }

  const binDir = path.join(__dirname, '..', 'bin');
  // NOTE: the packaged bin/ccql is a placeholder shim; the download below
  // replaces it with the real binary. Do not skip when the path exists.
  const binPath = path.join(binDir, 'ccql');

  // Release assets are .tar.xz archives containing ccql-<target>/ccql
  const assetName = `ccql-${target}.tar.xz`;
  const downloadUrl = `https://github.com/${REPO}/releases/download/v${VERSION}/${assetName}`;

  console.log(`Downloading ccql v${VERSION} for ${target}...`);

  const tmpDir = path.join(__dirname, '..', '.tmp');
  fs.mkdirSync(tmpDir, { recursive: true });
  fs.mkdirSync(binDir, { recursive: true });

  const archivePath = path.join(tmpDir, assetName);

  try {
    await downloadFile(downloadUrl, archivePath);

    execSync(`tar -xJf "${archivePath}" -C "${tmpDir}"`);
    fs.copyFileSync(path.join(tmpDir, `ccql-${target}`, 'ccql'), binPath);
    fs.chmodSync(binPath, 0o755);
    console.log('ccql installed successfully!');
  } catch (err) {
    console.error('Failed to install ccql:', err.message);
    console.error('Please install from source: cargo install ccql');
    process.exit(1);
  } finally {
    // Cleanup
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

main();
