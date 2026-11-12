import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const cache = join(root, '.local', 'windows');
const binaries = join(root, 'src-tauri', 'binaries');
const resources = join(root, 'src-tauri', 'resources');
const licenses = join(resources, 'windows-licenses');
const tag = 'autobuild-2026-11-11-12-39';
const archiveName = 'ffmpeg-n8.1.3-35-gf1acb0a266-win64-lgpl-8.1.zip';
const release = `https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/tag/${tag}`;
const download = `https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/${tag}/${archiveName}`;
const font = 'NotoSansCJKsc-Regular.otf';
const fontProject = 'https://raw.githubusercontent.com/notofonts/noto-cjk/Sans2.004/Sans';

function run(command, args) {
  const result = spawnSync(command, args, { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with status ${result.status}`);
}

async function digest(path) {
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(path)) hash.update(bytes);
  return hash.digest('hex');
}

async function fetchFile(url, path, checksum) {
  if (await stat(path).catch(() => null)) {
    if (await digest(path) === checksum) return;
    throw new Error(`Checksum mismatch: ${path}`);
  }
  const temporary = `${path}.download`;
  try {
    run('curl', ['--fail', '--location', '--retry', '2', '--show-error', '--output', temporary, url]);
    if (await digest(temporary) !== checksum) throw new Error(`Checksum mismatch: ${url}`);
    await rename(temporary, path);
  } finally {
    await rm(temporary, { force: true });
  }
}

await Promise.all([cache, binaries, licenses].map(path => mkdir(path, { recursive: true })));
const archive = join(cache, 'ffmpeg-win64.zip');
await fetchFile(download, archive, 'bb72a907fa5d6ef0793f46710284e6a55164716317bacb0ab2db1632964c7332');
const extracted = join(cache, 'ffmpeg-win64');
await mkdir(extracted, { recursive: true });
run('tar', ['-xf', archive, '-C', extracted]);
const distribution = join(extracted, archiveName.slice(0, -4));
for (const name of ['ffmpeg', 'ffprobe']) {
  await copyFile(join(distribution, 'bin', `${name}.exe`), join(binaries, `${name}-x86_64-pc-windows-msvc.exe`));
}
for (const entry of await readdir(distribution, { withFileTypes: true })) {
  if (entry.isFile() && /^(copying|license|readme)/i.test(entry.name)) {
    await copyFile(join(distribution, entry.name), join(licenses, `FFmpeg-${entry.name}`));
  }
}
await writeFile(join(licenses, 'FFmpeg-build.txt'), [
  'FFmpeg and FFprobe are separate executables built by FFmpeg-Builds.',
  `Binary release, source links and build details: ${release}`,
  'Corresponding FFmpeg revision: https://github.com/FFmpeg/FFmpeg/tree/f1acb0a266',
  'Build scripts and dependency revisions: https://github.com/FFmpeg-Builds/FFmpeg-Builds',
  '',
].join('\n'));
await fetchFile(`${fontProject}/OTF/SimplifiedChinese/${font}`, join(resources, font),
  '2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b');
await fetchFile('https://raw.githubusercontent.com/notofonts/noto-cjk/main/Sans/LICENSE', join(licenses, 'Noto-OFL.txt'),
  '6a73f9541c2de74158c0e7cf6b0a58ef774f5a780bf191f2d7ec9cc53efe2bf2');
