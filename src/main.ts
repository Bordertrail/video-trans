import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open, save } from '@tauri-apps/plugin-dialog';
import { version } from '../src-tauri/tauri.conf.json';
import { bindAttribute, bindText, locale, setLanguage, t, translatePage } from './i18n';
import { installLibrary } from './library';
import { directoryPrefix, fileName, inDirectory, isWindows } from './paths';
import './style.css';
import { installTrimTimeline, totalClipDuration, trimMarkup, type Clip } from './trim';

type Format = 'mp4' | 'webm' | 'mov';
type Resolution = 'original' | '1080p' | '720p' | '480p';
type Size = { width: number; height: number };
type AppError = { code: string; message: string; detail?: string };
type Media = {
  mediaId: number; name: string; path: string; bytes: number; duration: number | null;
  display: Size; hasAudio: boolean; unsupported: string | null;
};
type Plan = { size: Size | null; p1080: boolean; p720: boolean; p480: boolean; resolutionError: AppError | null; targetSizeError: AppError | null; clipError: AppError | null };
type PresetPosition = 'topLeft' | 'topRight' | 'bottomLeft' | 'bottomRight' | 'center';
type Point = { x: number; y: number };
type Position = PresetPosition | Point;
type MarkRect = Point & Size;
type Watermark =
  | { kind: 'off' }
  | { kind: 'text'; text: string; fontSize: number; color: string; opacity: number; position: Position }
  | { kind: 'image'; path: string; widthPercent: number; opacity: number; position: Position };
type Preset = { format: Format; resolution: Resolution; targetBytes: number | null; watermark: Watermark };
type Settings = Preset & { mediaId: number; clips: Clip[] };
type Sample = { reference: string; compressed: string; start: number; end: number };
type ConversionEvent =
  | { type: 'running' | 'retrying' | 'finalizing' | 'cancelled' }
  | { type: 'progress'; percent: number | null }
  | { type: 'succeeded'; path: string; bytes: number }
  | { type: 'failed'; error: AppError };
type JobEvent = { jobId: number } & ConversionEvent;

document.querySelector('#app')!.innerHTML = `
  <header class="page-header">
    <div class="app-icon" aria-hidden="true"><span></span></div>
    <div><h1 data-i18n>视频转码</h1><p data-i18n>转换格式、调整分辨率和添加水印</p></div>
    <div class="header-actions">
      <div class="language-switch" role="radiogroup" aria-label="语言" data-i18n-attrs>
        <label lang="zh-CN">
          <input type="radio" name="language" value="zh-CN" aria-label="简体中文">
          <span aria-hidden="true">中文</span>
        </label>
        <label lang="en">
          <input type="radio" name="language" value="en" aria-label="English">
          <span aria-hidden="true">EN</span>
        </label>
      </div>
      <button type="button" class="button secondary" id="about-button" aria-haspopup="dialog" data-i18n>关于</button>
    </div>
  </header>
  <main>
    <div class="preview-column">
      <section class="workspace" aria-labelledby="video-title">
        <header class="workspace-head">
          <h2 id="video-title" data-i18n>视频预览</h2>
          <div class="workspace-actions">
            <button type="button" class="button subtle" id="trim-button" aria-controls="trim-timeline" aria-expanded="false" hidden data-i18n>截取片段</button>
            <button type="button" class="button subtle" id="trim-restore" hidden data-i18n>恢复全片</button>
            <button type="button" class="button secondary" id="workspaces" data-i18n>工作空间</button>
            <button type="button" class="button subtle" id="replace" hidden data-i18n>更换视频</button>
          </div>
        </header>
        <div class="viewer">
          <div id="preview-modes" class="viewer-toolbar" hidden>
            <div class="preview-modes" role="group" aria-label="预览模式" data-i18n-attrs>
              <button type="button" id="play-source" aria-pressed="false" data-i18n>源视频</button>
              <button type="button" id="still-preview" aria-pressed="true" data-i18n>成片预览</button>
              <button type="button" id="play-result" aria-pressed="false" hidden data-i18n>已导出</button>
            </div>
          </div>
          <div class="viewer-stage">
            <div id="drop-zone" class="drop-zone">
              <div class="film-icon" aria-hidden="true"><span></span></div>
              <strong data-i18n>拖入一个视频文件</strong>
              <span data-i18n>支持 MP4、MOV、MKV、WebM 等格式</span>
              <button type="button" id="choose" class="button primary" data-i18n>选择视频</button>
            </div>
            <div id="preview" class="preview" hidden>
              <div id="preview-surface" class="preview-surface" hidden>
                <img id="preview-image" alt="成品画面预览" draggable="false" data-i18n-attrs>
                <img id="watermark-layer" class="watermark-layer" alt="水印，可拖动或用方向键调整位置" tabindex="0" draggable="false" hidden data-i18n-attrs>
              </div>
              <p id="preview-status" role="status" data-i18n>正在生成预览…</p>
              <button type="button" id="preview-movie" class="preview-play" aria-label="播放预览" data-i18n-attrs hidden>
                <span aria-hidden="true"></span>
              </button>
            </div>
            <div id="playback-panel" class="preview playback" hidden>
              <video id="video-player" controls playsinline preload="metadata" aria-label="视频播放器" hidden data-i18n-attrs></video>
              <p id="playback-status" role="status" hidden></p>
            </div>
          </div>
          ${trimMarkup}
          <footer id="viewer-file" class="viewer-file" hidden>
            <span class="file-icon" aria-hidden="true"></span>
            <div class="file-info">
              <strong id="filename"></strong>
              <span id="metadata"></span>
            </div>
          </footer>
        </div>
        <p id="input-error" class="message error" role="alert" hidden></p>
      </section>
      <section class="task" id="task" aria-labelledby="task-title" aria-live="polite">
        <div class="task-summary">
          <div class="task-body">
            <strong id="task-title" data-i18n>等待转换</strong>
            <p id="task-detail"></p>
          </div>
          <div class="task-actions">
            <button type="button" class="button secondary" id="cancel" hidden data-i18n>取消</button>
            <button type="button" class="button secondary" id="open-result" hidden data-i18n>打开文件</button>
            <button type="button" class="button secondary" id="reveal-result" hidden data-i18n>${isWindows ? '在资源管理器中显示' : '在访达中显示'}</button>
          </div>
        </div>
        <progress id="progress" aria-label="转换进度" max="100" value="0" data-i18n-attrs></progress>
        <details id="diagnostics" hidden><summary data-i18n>诊断信息</summary><pre id="diagnostic-text"></pre></details>
      </section>
    </div>
    <section class="settings" aria-labelledby="settings-title">
      <header class="settings-heading">
        <h2 id="settings-title" data-i18n>输出设置</h2>
        <button type="button" class="button subtle" id="presets-button" aria-haspopup="dialog" data-i18n>常用预设</button>
      </header>
      <div class="settings-scroll">
        <fieldset id="controls">
          <label class="field" for="format"><span data-i18n>格式</span>
            <select id="format"><option value="mp4">MP4</option><option value="webm">WebM</option><option value="mov">MOV</option></select>
          </label>
          <label class="field" for="resolution"><span data-i18n>分辨率</span>
            <select id="resolution"><option value="original" data-i18n>保持原分辨率</option><option value="1080p">1080p</option><option value="720p">720p</option><option value="480p">480p</option></select>
          </label>
          <p id="dimensions" class="hint" hidden></p>
          <label class="field" for="target-size"><span data-i18n>目标大小上限（MB）</span>
            <input id="target-size" type="number" min="1" step="any" placeholder="自动" aria-describedby="target-size-hint" data-i18n-attrs>
            <span id="target-size-hint" class="hint" hidden></span>
          </label>
          <div class="field">
            <span data-i18n>导出目录</span>
            <button type="button" class="button secondary directory-control" id="output-dirs" aria-label="管理导出目录" data-i18n-attrs>
              <span class="truncate" id="export-directory-summary" data-i18n>源视频目录</span><span data-i18n>管理</span>
            </button>
          </div>
          <div class="watermark">
            <label class="field" for="watermark"><span data-i18n>水印</span>
              <select id="watermark"><option value="off" data-i18n>关闭水印</option><option value="text" data-i18n>文字水印</option><option value="image" data-i18n>图片水印</option></select>
            </label>
            <div id="text-fields" class="fields" hidden>
              <label class="field" for="text"><span data-i18n>文字</span> <input id="text" type="text" placeholder="输入水印文字" data-i18n-attrs></label>
              <div class="split">
                <label class="field" for="font-size"><span data-i18n>字号（px）</span> <input id="font-size" type="number" min="12" max="200" value="100"></label>
                <label class="field" for="color"><span data-i18n>颜色</span> <input id="color" type="color" value="#ffffff"></label>
              </div>
            </div>
            <div id="image-fields" class="fields" hidden>
              <button type="button" class="button secondary" id="choose-image" data-i18n>选择 PNG 或 JPEG</button>
              <span id="image-name" class="hint truncate" hidden></span>
              <label class="field" for="image-width"><span data-i18n>宽度（画面的 %）</span>
                <input id="image-width" type="number" min="5" max="60" value="20">
              </label>
            </div>
            <div id="common-fields" class="fields" hidden>
              <div class="split">
                <label class="field" for="position"><span data-i18n>位置</span>
                  <select id="position">
                    <option value="bottomRight" data-i18n>右下角</option><option value="bottomLeft" data-i18n>左下角</option>
                    <option value="topRight" data-i18n>右上角</option><option value="topLeft" data-i18n>左上角</option><option value="center" data-i18n>居中</option>
                    <option id="custom-position" value="custom" disabled data-i18n>自定义</option>
                  </select>
                </label>
                <label class="field" for="opacity"><span data-i18n>不透明度（%）</span> <input id="opacity" type="number" min="10" max="100" value="70"></label>
              </div>
              <p class="hint" data-i18n>可在预览中直接拖动水印。</p>
            </div>
          </div>
        </fieldset>
        <p id="settings-error" class="message error" role="alert" hidden></p>
      </div>
      <div class="export-actions">
        <button type="button" id="sample-button" class="button secondary" disabled data-i18n>压缩预览</button>
        <button type="button" id="start" class="button primary" disabled data-i18n>开始转换</button>
      </div>
    </section>
  </main>
  <dialog id="about" class="about-dialog" aria-labelledby="about-title">
    <header class="about-heading">
      <div class="app-icon" aria-hidden="true">
        <span></span>
        <small class="about-signature">曦</small>
      </div>
      <h2 id="about-title" data-i18n>关于视频转码</h2>
    </header>
    <dl class="about-details">
      <div>
        <dt data-i18n>开发者</dt>
        <dd class="about-developer">
          Bordertrail
          <a id="about-github" class="about-github" href="https://github.com/Bordertrail/video-trans"
             target="_blank" rel="noopener noreferrer" aria-label="在浏览器中打开 GitHub 项目仓库" title="GitHub 项目仓库">
            <svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">
              <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38l-.01-1.49c-2.23.48-2.7-.95-2.7-.95-.36-.92-.89-1.16-.89-1.16-.73-.5.05-.49.05-.49.8.06 1.23.82 1.23.82.71 1.22 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.22 2.2.82a7.65 7.65 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.28.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48l-.01 2.2c0 .21.15.46.55.38A8 8 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/>
            </svg>
          </a>
        </dd>
      </div>
      <div><dt data-i18n>版本</dt><dd>${version}</dd></div>
      <div><dt data-i18n>开发日期</dt><dd><time datetime="2026-11-12" data-i18n>2026 年 11 月 12 日</time></dd></div>
    </dl>
    <form method="dialog" class="dialog-actions">
      <button class="button secondary" autofocus data-i18n>关闭</button>
    </form>
  </dialog>
  <dialog id="presets-dialog" aria-labelledby="presets-title">
    <h2 id="presets-title" data-i18n>常用预设</h2>
    <div id="preset-list" class="library-list preset-list"></div>
    <form id="preset-form" class="preset-form">
      <label class="field"><span data-i18n>预设名称</span><input id="preset-name" type="text" maxlength="60" required></label>
      <button class="button secondary" type="submit" data-i18n>保存当前设置</button>
    </form>
    <p id="preset-status" class="message" role="status" hidden></p>
    <form method="dialog" class="dialog-actions"><button class="button secondary" data-i18n>关闭</button></form>
  </dialog>
  <dialog id="sample-dialog" class="sample-dialog" aria-labelledby="sample-title">
    <header class="library-heading">
      <h2 id="sample-title" data-i18n>压缩预览</h2>
      <form method="dialog"><button class="button secondary" data-i18n>关闭</button></form>
    </header>
    <p id="sample-status" role="status"></p>
    <div id="sample-players" class="split" hidden>
      <div><h3 data-i18n>原片参考</h3><video id="sample-reference" controls playsinline preload="metadata"></video></div>
      <div><h3 data-i18n>压缩样片</h3><video id="sample-compressed" controls playsinline preload="metadata"></video></div>
    </div>
  </dialog>
  <dialog id="confirmation">
    <form method="dialog">
      <h2 id="confirm-title"></h2>
      <p id="confirm-body"></p>
      <div class="dialog-actions">
        <button class="button secondary" value="cancel" autofocus data-i18n>取消</button>
        <button class="button primary" id="confirm-action" value="confirm" data-i18n>继续</button>
      </div>
    </form>
  </dialog>`;

const element = <T extends HTMLElement>(id: string) => document.getElementById(id)! as T;
const button = (id: string) => element<HTMLButtonElement>(id);
const select = (id: string) => element<HTMLSelectElement>(id);
const input = (id: string) => element<HTMLInputElement>(id);
const controls = element<HTMLFieldSetElement>('controls');
const preview = element<HTMLImageElement>('preview-image');
const previewPanel = element('preview');
const surface = element('preview-surface');
const layer = element<HTMLImageElement>('watermark-layer');
const player = element<HTMLVideoElement>('video-player');
const progress = element<HTMLProgressElement>('progress');
const confirmation = element<HTMLDialogElement>('confirmation');

let media: Media | null = null;
let selectionId = 0;
let previewId = 0;
let previewTimer: ReturnType<typeof setTimeout> | undefined;
let previewUrl: string | undefined;
let watermarkUrl: string | undefined;
let frameSize: Size | null = null;
let mark: MarkRect | null = null;
let customPosition: Point | null = null;
let drag: { pointer: number; start: Point; origin: Point } | null = null;
let imagePath = '';
let busy = false;
let jobId: number | null = null;
let canStart = false;
let playbackId = 0;
let playing: 'source' | 'result' | 'preview' | null = null;
let readingVideo = false;
let result: { path: string; bytes: number; size: Size | null } | null = null;
let clips: Clip[] = [];
let sampleRequest = 0;

function errorFrom(error: unknown): AppError {
  if (typeof error === 'object' && error !== null && 'message' in error && 'code' in error) {
    return error as AppError;
  }
  return { code: 'Unknown', message: '操作失败，请重试。', detail: String(error) };
}

function message(id: string, text: string | (() => string) = '') {
  const node = element(id);
  bindText(node, text);
  node.hidden = !node.textContent;
}

function bytes(size: number): string {
  if (size < 1_000) return `${size} B`;
  if (size < 1_000_000) return `${(size / 1_000).toFixed(1)} KB`;
  if (size < 1_000_000_000) return `${(size / 1_000_000).toFixed(2)} MB`;
  return `${(size / 1_000_000_000).toFixed(2)} GB`;
}

function secondsLabel(seconds: number) {
  return new Intl.NumberFormat(locale(), { maximumFractionDigits: 3 }).format(seconds) + ' s';
}

function time(duration: number | null): string {
  if (duration === null) return '时长未知';
  const seconds = Math.round(duration);
  const parts = [Math.floor(seconds / 3600), Math.floor(seconds / 60) % 60, seconds % 60];
  return parts.slice(parts[0] > 0 ? 0 : 1).map(value => String(value).padStart(2, '0')).join(':');
}

function exportTimestamp(date: Date): string {
  const day = [date.getMonth() + 1, date.getDate()];
  const time = [date.getHours(), date.getMinutes(), date.getSeconds()];
  const twoDigits = (value: number) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${day.map(twoDigits).join('-')}_${time.map(twoDigits).join('-')}`;
}

function presetSettings(): Preset {
  const kind = select('watermark').value;
  const opacity = input('opacity').valueAsNumber;
  const value = select('position').value;
  const position: Position = value === 'custom' ? (customPosition ?? 'bottomRight') : value as PresetPosition;
  const watermark: Watermark = kind === 'text'
    ? { kind, text: input('text').value, fontSize: input('font-size').valueAsNumber, color: input('color').value, opacity, position }
    : kind === 'image'
      ? { kind, path: imagePath, widthPercent: input('image-width').valueAsNumber, opacity, position }
      : { kind: 'off' };
  return {
    format: select('format').value as Format,
    resolution: select('resolution').value as Resolution,
    targetBytes: input('target-size').value === '' ? null : Math.round(input('target-size').valueAsNumber * 1_000_000),
    watermark,
  };
}

function settings(): Settings | null {
  if (!media) return null;
  return { ...presetSettings(), mediaId: media.mediaId, clips };
}

function syncTargetSize() {
  const target = input('target-size');
  const maximum = media ? media.bytes / 1_000_000 : 0;
  target.max = media ? String(maximum) : '';
  bindAttribute(target, 'placeholder', !media ? '自动' : maximum < 1 ? '源文件小于 1 MB' : !media.duration ? '视频时长未知' : '自动');
  const displayedMax = new Intl.NumberFormat(locale(), { maximumFractionDigits: 3 }).format(Math.floor(maximum * 1000) / 1000);
  message('target-size-hint', !media || maximum < 1 || !media.duration ? '' : () => t('可填 1–{max} MB，留空为自动。', { max: displayedMax }));
}

function targetSizeInputError() {
  const field = input('target-size');
  return !field.validity.valid ? '请输入范围内的目标大小，或清空以使用自动模式。' : '';
}

function watermarkError(mark: Watermark): string {
  if (mark.kind === 'off') return '';
  const valid = (n: number, low: number, high: number) => Number.isInteger(n) && n >= low && n <= high;
  if (!valid(mark.opacity, 10, 100)) return '请输入 10 到 100 之间的不透明度。';
  if (mark.kind === 'text') {
    if (!mark.text.trim() || [...mark.text.trim()].length > 80) return '请输入 1 到 80 个字符的水印文字。';
    if (!valid(mark.fontSize, 12, 200)) return '请输入 12 到 200 之间的字号。';
  } else {
    if (!mark.path) return '请选择水印图片。';
    if (!valid(mark.widthPercent, 5, 60)) return '请输入 5 到 60 之间的图片宽度。';
  }
  return '';
}

function clearPreview() {
  endDrag();
  if (previewUrl) URL.revokeObjectURL(previewUrl);
  if (watermarkUrl) URL.revokeObjectURL(watermarkUrl);
  previewUrl = undefined;
  watermarkUrl = undefined;
  frameSize = null;
  mark = null;
  preview.removeAttribute('src');
  layer.removeAttribute('src');
  layer.hidden = true;
  surface.hidden = true;
}

function fitPreview() {
  if (!frameSize) return;
  const scale = Math.min(previewPanel.clientWidth / frameSize.width, previewPanel.clientHeight / frameSize.height);
  surface.style.width = `${frameSize.width * scale}px`;
  surface.style.height = `${frameSize.height * scale}px`;
}

function drawMark() {
  if (!frameSize || !mark) return;
  layer.style.left = `${mark.x / frameSize.width * 100}%`;
  layer.style.top = `${mark.y / frameSize.height * 100}%`;
  layer.style.width = `${mark.width / frameSize.width * 100}%`;
  layer.style.height = `${mark.height / frameSize.height * 100}%`;
}

function showPreview(data: ArrayBuffer, size: Size) {
  // Header: JPEG length, x, y, width, height (u32 LE), then JPEG and optional PNG.
  const header = new DataView(data, 0, 20);
  const jpegEnd = 20 + header.getUint32(0, true);
  const width = header.getUint32(12, true);
  const height = header.getUint32(16, true);
  previewUrl = URL.createObjectURL(new Blob([data.slice(20, jpegEnd)], { type: 'image/jpeg' }));
  preview.src = previewUrl;
  frameSize = size;
  if (width && height) {
    mark = { x: header.getUint32(4, true), y: header.getUint32(8, true), width, height };
    watermarkUrl = URL.createObjectURL(new Blob([data.slice(jpegEnd)], { type: 'image/png' }));
    layer.src = watermarkUrl;
    drawMark();
    layer.hidden = false;
  }
  fitPreview();
  surface.hidden = false;
}

function moveMark(x: number, y: number) {
  if (!frameSize || !mark) return;
  const maxX = frameSize.width - mark.width;
  const maxY = frameSize.height - mark.height;
  const nextX = Math.min(maxX, Math.max(0, Math.round(x)));
  const nextY = Math.min(maxY, Math.max(0, Math.round(y)));
  if (nextX === mark.x && nextY === mark.y) return;
  customPosition = { x: maxX ? nextX / maxX : 0, y: maxY ? nextY / maxY : 0 };
  element<HTMLOptionElement>('custom-position').disabled = false;
  select('position').value = 'custom';
  mark.x = nextX;
  mark.y = nextY;
  drawMark();
}

function endDrag() {
  const pointer = drag?.pointer;
  drag = null;
  layer.classList.remove('dragging');
  if (pointer !== undefined && layer.hasPointerCapture(pointer)) layer.releasePointerCapture(pointer);
}

function setBusy(value: boolean) {
  busy = value;
  if (value) endDrag();
  layer.tabIndex = value ? -1 : 0;
  controls.disabled = value;
  button('replace').disabled = value;
  button('workspaces').disabled = value;
  button('output-dirs').disabled = value;
  button('choose').disabled = value;
  button('trim-button').disabled = value;
  button('trim-restore').disabled = value;
  setCanStart(canStart);
}

function syncPreviewPlay() {
  button('preview-movie').hidden = !media || playing !== null || busy || trim.active || !canStart;
}

function setCanStart(value: boolean) {
  canStart = value;
  button('start').disabled = busy || trim.active || !value;
  button('sample-button').disabled = busy || trim.active || !value || !media?.duration;
  syncPreviewPlay();
}

function schedulePreview() {
  if (playing === 'preview') showStillPreview();
  const requestId = ++previewId;
  clearTimeout(previewTimer);
  setCanStart(false);
  previewTimer = setTimeout(() => void refreshPreview(requestId), 250);
}

async function refreshPreview(requestId: number) {
  const current = settings();
  if (!current) return;
  const isCurrent = () => requestId === previewId && media?.mediaId === current.mediaId;
  try {
    let plan = await invoke<Plan>('get_export_plan', { settings: { ...current, watermark: { kind: 'off' } } });
    if (!isCurrent()) return;
    const options = select('resolution').options;
    options[1].disabled = !plan.p1080;
    options[2].disabled = !plan.p720;
    options[3].disabled = !plan.p480;
    if (!media?.unsupported && current.resolution !== 'original' && options[select('resolution').selectedIndex].disabled) {
      select('resolution').value = 'original';
      current.resolution = 'original';
      plan = await invoke<Plan>('get_export_plan', { settings: { ...current, watermark: { kind: 'off' } } });
      if (!isCurrent()) return;
    }
    const size = plan.size;
    message('dimensions', size ? () => t('输出尺寸 {width} × {height}', size) : '');
    const validation = plan.resolutionError?.message || plan.clipError?.message || watermarkError(current.watermark);
    const sizeError = targetSizeInputError() || plan.targetSizeError?.message || '';
    message('settings-error', validation || sizeError);
    if (validation || !plan.size) {
      clearPreview();
      message('preview-status', '调整输出设置后可预览画面。');
      return;
    }

    clearPreview();
    message('preview-status', '正在生成预览…');
    const data = await invoke<ArrayBuffer>('render_preview', { settings: current, requestId });
    if (!isCurrent()) return;
    showPreview(data, plan.size);
    message('preview-status');
    setCanStart(!sizeError);
    syncViewer();
  } catch (reason) {
    if (!isCurrent()) return;
    const error = errorFrom(reason);
    if (error.code === 'InvalidWatermark') {
      message('settings-error', error.message);
      message('preview-status', '调整水印设置后可预览画面。');
    } else {
      message('preview-status', error.message);
      setCanStart(error.code === 'PreviewFailed' && element('settings-error').hidden);
    }
  }
}

function syncViewer() {
  const showingResult = playing === 'result';
  const showingMovie = playing === 'preview';
  const onPreviewTab = playing === null || showingMovie;
  const file = showingResult ? result : media;
  const filename = showingResult && result ? fileName(result.path) : media?.name;
  const size = showingResult ? result?.size : onPreviewTab ? frameSize ?? media?.display : media?.display;
  const info = [];
  if (size) info.push(`${size.width} × ${size.height}`);
  if (showingMovie) info.push(select('format').value.toUpperCase());
  if (!showingResult && media && !(showingMovie && clips.length)) info.push(t(time(media.duration)));
  if (file && !showingMovie) info.push(bytes(file.bytes));
  if (!showingResult && media && !media.hasAudio) info.push(t('无音轨'));
  if (!showingResult && clips.length) {
    info.push(t('已选 {count} 段 · 共 {duration}', {
      count: clips.length, duration: secondsLabel(totalClipDuration(clips)),
    }));
  }

  element('viewer-file').hidden = !file;
  element('filename').textContent = filename ?? '';
  element('filename').title = filename ?? '';
  element('metadata').textContent = info.join(' · ');

  element('preview-modes').hidden = !media && !result;
  button('play-source').hidden = !media;
  button('still-preview').hidden = !media;
  button('play-result').hidden = !result;
  button('play-source').setAttribute('aria-pressed', String(playing === 'source'));
  button('still-preview').setAttribute('aria-pressed', String(onPreviewTab));
  button('play-result').setAttribute('aria-pressed', String(showingResult));
  syncPreviewPlay();

  button('replace').hidden = !media && !readingVideo && playing === null;
  button('trim-button').hidden = !media?.duration || media.duration < 0.1;
  button('trim-restore').hidden = !clips.length || trim.active;
  bindText(button('replace'), media || readingVideo ? '更换视频' : '选择视频');
  element('drop-zone').hidden = Boolean(media || readingVideo || playing);
  previewPanel.hidden = playing !== null || (!media && !readingVideo);
  element('playback-panel').hidden = playing === null;
  trim.sync();
}

function resetPlayer() {
  player.onerror = null;
  player.onloadeddata = null;
  player.pause();
  player.removeAttribute('src');
  player.load();
  player.hidden = true;
}

function showStillPreview() {
  trim.close();
  const id = ++playbackId;
  if (playing) void invoke('stop_playback', { requestId: id }).catch(() => {});
  playing = null;
  resetPlayer();
  syncViewer();
}

function playOutputPreview() {
  const current = settings();
  if (!current || busy || !canStart || trim.active) return;
  const restoreFocus = document.activeElement === button('preview-movie');
  const requestId = ++playbackId;
  playing = 'preview';
  resetPlayer();
  syncViewer();
  if (restoreFocus) button('still-preview').focus({ preventScroll: true });
  bindAttribute(player, 'aria-label', '成片预览播放器');
  const isCurrent = () => requestId === playbackId && playing === 'preview';
  const types = {
    mp4: 'video/mp4; codecs="avc1.42E01E"',
    mov: 'video/quicktime',
    webm: 'video/webm; codecs="vp9"',
  };

  async function load(compatible: boolean) {
    const onEvent = new Channel<ConversionEvent>();
    let retrying = false;
    message('playback-status', '正在生成成片预览…');
    onEvent.onmessage = event => {
      if (!isCurrent()) return;
      if (event.type === 'retrying') retrying = true;
      if (event.type === 'progress') {
        const percent = event.percent;
        const label = retrying ? '正在降低码率后重试' : '正在生成成片预览…';
        message('playback-status', percent === null ? label : () => `${t(label)} ${percent}%`);
      }
    };
    try {
      const url = await invoke<string>('output_preview_url', { settings: current, requestId, compatible, onEvent });
      if (!isCurrent()) return;
      player.onerror = () => {
        if (!isCurrent()) return;
        resetPlayer();
        if (compatible) message('playback-status', '无法播放此视频。');
        else void load(true);
      };
      player.onloadeddata = () => {
        if (isCurrent()) message('playback-status');
      };
      player.hidden = false;
      player.src = url;
      void player.play().catch(() => {});
    } catch (reason) {
      if (isCurrent()) message('playback-status', errorFrom(reason).message);
    }
  }
  void load(!player.canPlayType(types[current.format]));
}

function playMedia(kind: 'source' | 'result', autoplay = true) {
  const mediaId = kind === 'source' ? media?.mediaId : null;
  if (mediaId === undefined || (kind === 'result' && !result)) return;
  if (kind === 'result') trim.close();
  if (playing === kind && player.hasAttribute('src')) {
    if (autoplay && !trim.active) void player.play().catch(() => {});
    return;
  }

  const requestId = ++playbackId;
  playing = kind;
  resetPlayer();
  syncViewer();
  bindAttribute(player, 'aria-label', kind === 'source' ? '源视频播放器' : '成品播放器');

  let fallback = false;
  async function load(compatible: boolean) {
    resetPlayer();
    message('playback-status', compatible ? '正在准备兼容播放版本…' : '正在打开视频…');
    try {
      const url = await invoke<string | null>('playback_url', { mediaId, requestId, compatible });
      if (requestId !== playbackId) return;
      if (!url) {
        fallback = true;
        void load(true);
        return;
      }
      player.onerror = () => {
        if (requestId !== playbackId) return;
        if (!fallback) {
          fallback = true;
          void load(true);
        } else {
          resetPlayer();
          message('playback-status', '无法播放此视频。');
        }
      };
      player.onloadeddata = () => {
        if (requestId !== playbackId) return;
        message('playback-status');
      };
      player.src = url;
      player.hidden = false;
      if (autoplay && !trim.active) void player.play().catch(() => {});
    } catch (reason) {
      if (requestId !== playbackId) return;
      message('playback-status', errorFrom(reason).message);
    }
  }
  void load(false);
}

async function loadVideo(path: string) {
  if (busy) return;
  const requestId = ++selectionId;
  ++previewId;
  readingVideo = true;
  media = null;
  clips = [];
  syncTargetSize();
  showStillPreview();
  clearPreview();
  setCanStart(false);
  message('input-error');
  message('settings-error');
  message('dimensions');
  message('preview-status', '正在读取视频…');
  try {
    const selected = await invoke<Media>('probe_video', { path, requestId });
    if (requestId !== selectionId) return;
    media = selected;
    syncTargetSize();
    readingVideo = false;
    syncViewer();
    schedulePreview();
  } catch (reason) {
    if (requestId !== selectionId) return;
    readingVideo = false;
    syncViewer();
    message('input-error', errorFrom(reason).message);
  }
}

async function chooseVideo() {
  try {
    const path = await open({ title: t('选择视频'), multiple: false, directory: false });
    if (typeof path === 'string') await loadVideo(path);
  } catch (reason) {
    message('input-error', errorFrom(reason).message);
  }
}

async function chooseImage() {
  try {
    const path = await open({
      title: t('选择水印图片'),
      filters: [{ name: t('PNG 或 JPEG'), extensions: ['png', 'jpeg', 'jpg'] }],
    });
    if (typeof path === 'string') {
      imagePath = path;
      message('image-name', fileName(path));
      schedulePreview();
    }
  } catch (reason) {
    message('settings-error', errorFrom(reason).message);
  }
}

function confirm(title: string, body: string, action: string): Promise<boolean> {
  if (confirmation.open) return Promise.resolve(false);
  bindText(element('confirm-title'), title);
  bindText(element('confirm-body'), body);
  bindText(button('confirm-action'), action);
  confirmation.returnValue = 'cancel';
  confirmation.showModal();
  return new Promise(resolve => {
    confirmation.addEventListener('close', () => resolve(confirmation.returnValue === 'confirm'), { once: true });
  });
}

type NamedPreset = Preset & { name: string };

function installPresets() {
  const storageKey = 'video-trans-presets';
  const dialog = element<HTMLDialogElement>('presets-dialog');
  const list = element('preset-list');
  const nameField = input('preset-name');
  let presets: NamedPreset[] = [];
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(storageKey) ?? '[]');
    if (Array.isArray(saved)) {
      presets = saved.filter(preset => typeof preset?.name === 'string'
        && ['mp4', 'mov', 'webm'].includes(preset.format)
        && ['original', '1080p', '720p', '480p'].includes(preset.resolution)
        && ['off', 'text', 'image'].includes(preset.watermark?.kind));
    }
  } catch {
    // A broken preferences entry should not prevent the app from opening.
  }

  function savePresets(next: NamedPreset[]) {
    try {
      localStorage.setItem(storageKey, JSON.stringify(next));
      presets = next;
      render();
      return true;
    } catch {
      message('preset-status', '无法保存预设。');
      return false;
    }
  }

  function apply(preset: NamedPreset) {
    select('format').value = preset.format;
    select('resolution').value = preset.resolution;
    input('target-size').value = preset.targetBytes == null ? '' : String(preset.targetBytes / 1_000_000);
    const mark = preset.watermark;
    select('watermark').value = mark.kind;
    if (mark.kind !== 'off') {
      customPosition = typeof mark.position === 'object' ? mark.position : null;
      element<HTMLOptionElement>('custom-position').disabled = customPosition === null;
      select('position').value = typeof mark.position === 'string' ? mark.position : 'custom';
      input('opacity').value = String(mark.opacity);
    }
    if (mark.kind === 'text') {
      input('text').value = mark.text;
      input('font-size').value = String(mark.fontSize);
      input('color').value = mark.color;
    } else if (mark.kind === 'image') {
      imagePath = mark.path;
      input('image-width').value = String(mark.widthPercent);
      message('image-name', () => fileName(mark.path));
    }
    select('watermark').dispatchEvent(new Event('change'));
    dialog.close();
    schedulePreview();
  }

  function render() {
    list.replaceChildren();
    if (!presets.length) {
      const empty = document.createElement('p');
      bindText(empty, '暂无预设。');
      list.append(empty);
    }
    for (const preset of presets) {
      const row = document.createElement('div');
      row.className = 'library-row';
      const summary = document.createElement('div');
      const name = document.createElement('strong');
      name.textContent = preset.name;
      const detail = document.createElement('span');
      bindText(detail, () => [
        preset.format.toUpperCase(),
        preset.resolution === 'original' ? t('保持原分辨率') : preset.resolution,
        preset.targetBytes == null ? t('自动') : bytes(preset.targetBytes),
      ].join(' · '));
      summary.append(name, detail);
      const actions = document.createElement('div');
      actions.className = 'library-actions';
      const use = document.createElement('button');
      use.className = 'button secondary';
      use.onclick = () => apply(preset);
      bindText(use, '应用');
      const remove = document.createElement('button');
      remove.className = 'button subtle';
      remove.onclick = () => { savePresets(presets.filter(entry => entry !== preset)); };
      bindText(remove, '移除');
      actions.append(use, remove);
      row.append(summary, actions);
      list.append(row);
    }
  }

  button('presets-button').onclick = () => {
    if (busy) return;
    nameField.value = '';
    message('preset-status');
    render();
    dialog.showModal();
  };
  element<HTMLFormElement>('preset-form').onsubmit = async event => {
    event.preventDefault();
    const name = nameField.value.trim();
    const preset = presetSettings();
    const size = preset.targetBytes;
    const sizeError = size !== null && (!Number.isSafeInteger(size) || size < 1_000_000)
      ? '请输入至少 1 MB 的目标大小，或留空。' : '';
    const error = !name ? '请输入预设名称。' : sizeError || targetSizeInputError() || watermarkError(preset.watermark);
    if (error) {
      message('preset-status', error);
      return;
    }
    if (presets.some(entry => entry.name === name)
      && !await confirm('替换同名预设？', '保存后会覆盖该预设。', '替换')) return;
    if (savePresets([...presets.filter(entry => entry.name !== name), { ...preset, name }])) {
      nameField.value = '';
      message('preset-status', '预设已保存。');
    }
  };
}

function installCompressionPreview() {
  const dialog = element<HTMLDialogElement>('sample-dialog');
  const reference = element<HTMLVideoElement>('sample-reference');
  const compressed = element<HTMLVideoElement>('sample-compressed');
  let resumeMain = false;
  for (const [first, second] of [[reference, compressed], [compressed, reference]]) {
    first.onplay = () => second.pause();
  }
  reference.onerror = compressed.onerror = () => {
    if (dialog.open) message('sample-status', '此设备无法播放其中一个样片。');
  };
  dialog.onclose = () => {
    const requestId = ++sampleRequest;
    for (const video of [reference, compressed]) {
      video.pause();
      video.removeAttribute('src');
      video.load();
    }
    void invoke('stop_compression_sample', { requestId }).catch(() => {});
    if (resumeMain) void player.play().catch(() => {});
  };
  button('sample-button').onclick = async () => {
    const current = settings();
    if (!current || busy || !canStart || dialog.open) return;
    const requestId = ++sampleRequest;
    resumeMain = !player.paused;
    player.pause();
    element('sample-players').hidden = true;
    message('sample-status', '正在生成压缩样片…');
    dialog.showModal();
    try {
      const sample = await invoke<Sample>('compression_sample', { settings: current, requestId });
      if (requestId !== sampleRequest || !dialog.open) return;
      reference.src = sample.reference;
      compressed.src = sample.compressed;
      element('sample-players').hidden = false;
      message('sample-status', () => t('样片取自 {start}–{end}，画质仅供参考。', {
        start: secondsLabel(sample.start), end: secondsLabel(sample.end),
      }));
    } catch (reason) {
      if (requestId === sampleRequest && dialog.open) message('sample-status', errorFrom(reason).message);
    }
  };
}

function showTaskDetail(detail: string, isResultPath = false) {
  message('task-detail', detail);
  const detailLine = element('task-detail');
  detailLine.classList.toggle('truncate', isResultPath);
  detailLine.title = isResultPath ? detail : '';
}

function showTask(title: string, detail = '', percent?: number | null) {
  message('task-title', title);
  showTaskDetail(detail, percent === 100);
  progress.hidden = percent === undefined;
  if (typeof percent === 'number') progress.value = percent;
  else progress.removeAttribute('value');
}

function hideResult() {
  result = null;
  if (playing === 'result') showStillPreview();
  else syncViewer();
  button('open-result').hidden = true;
  button('reveal-result').hidden = true;
  element('diagnostics').hidden = true;
}

function failed(error: AppError) {
  showTask('转换失败', error.message);
  if (error.detail) {
    element('diagnostic-text').textContent = error.detail;
    element('diagnostics').hidden = false;
  }
}

async function start() {
  const current = settings();
  if (!current || !media || busy || trim.active || !canStart) return;
  const source = media.path;
  const outputSize = frameSize;
  const sourceName = fileName(source);
  const extensionStart = sourceName.lastIndexOf('.');
  const stem = extensionStart > 0 ? sourceName.slice(0, extensionStart) : sourceName;
  const filename = `${stem}-converted_${exportTimestamp(new Date())}.${current.format}`;
  setBusy(true);
  let terminal = false;
  try {
    const directory = await library.exportDirectory();
    const path = await save({
      title: t('保存成品'),
      defaultPath: directory ? inDirectory(directory, filename) : directoryPrefix(source) + filename,
      filters: [{ name: current.format.toUpperCase(), extensions: [current.format] }],
    });
    if (!path) {
      setBusy(false);
      return;
    }
    hideResult();
    button('cancel').hidden = false;
    button('cancel').disabled = true;
    showTask('正在准备', '', null);
    const onEvent = new Channel<JobEvent>();
    let retrying = false;
    onEvent.onmessage = event => {
      jobId = event.jobId;
      if (event.type === 'running') showTask('正在转换', '', null);
      if (event.type === 'retrying') {
        retrying = true;
        showTask('正在降低码率后重试', '', null);
      }
      if (event.type === 'progress') showTask(retrying ? '正在降低码率后重试' : '正在转换', event.percent === null ? '' : `${event.percent}%`, event.percent);
      if (event.type === 'finalizing') {
        button('cancel').disabled = true;
        showTask('正在写入成品', '', null);
      }
      if (event.type === 'succeeded' || event.type === 'failed' || event.type === 'cancelled') {
        terminal = true;
        jobId = null;
        button('cancel').hidden = true;
        setBusy(false);
        if (event.type === 'succeeded') {
          showTask('转换完成', `${event.path} · ${bytes(event.bytes)}`, 100);
          result = { path: event.path, bytes: event.bytes, size: outputSize };
          syncViewer();
          button('open-result').hidden = false;
          button('reveal-result').hidden = false;
        }
        if (event.type === 'failed') failed(event.error);
        if (event.type === 'cancelled') showTask('已取消');
      }
    };
    const launch = (overwrite: boolean) => invoke<number>('start_export', { settings: current, path, overwrite, onEvent });
    let id: number;
    try {
      id = await launch(false);
    } catch (reason) {
      if (errorFrom(reason).code !== 'OutputConflict') throw reason;
      if (!await confirm('替换同名文件？', '转换成功后将替换现有文件。', '替换')) {
        showTask('已取消');
        button('cancel').hidden = true;
        setBusy(false);
        return;
      }
      id = await launch(true);
    }
    library.exportStarted();
    if (!terminal) {
      jobId = id;
      if (element('task-title').textContent !== t('正在写入成品')) button('cancel').disabled = false;
    }
  } catch (reason) {
    failed(errorFrom(reason));
    button('cancel').hidden = true;
    jobId = null;
    setBusy(false);
  }
}

const library = installLibrary(loadVideo, reason => errorFrom(reason).message, () => {
  if (player.paused || player.ended) return;
  const requestId = playbackId;
  player.pause();
  return () => {
    if (requestId === playbackId) void player.play().catch(() => {});
  };
});

button('about-button').onclick = () => element<HTMLDialogElement>('about').showModal();
function updateWindowTitle() {
  if ('__TAURI_INTERNALS__' in window) {
    void getCurrentWindow().setTitle(t('视频转码')).catch(() => {});
  }
}

document.querySelectorAll<HTMLInputElement>('input[name="language"]').forEach(option => {
  const language = option.value === 'en' ? 'en' : 'zh-CN';
  option.checked = language === locale();
  option.onchange = () => {
    if (!option.checked) return;
    setLanguage(language);
    syncTargetSize();
    syncViewer();
    library.updateLanguage();
    updateWindowTitle();
  };
});
const trim = installTrimTimeline(player, secondsLabel, next => {
  clips = next;
  showStillPreview();
  schedulePreview();
}, () => {
  button('trim-restore').hidden = !clips.length || trim.active;
  setCanStart(canStart);
});
button('trim-restore').onclick = () => {
  if (busy || !clips.length) return;
  clips = [];
  showStillPreview();
  button('trim-button').focus();
  schedulePreview();
};
button('trim-button').onclick = () => {
  if (busy || !media?.duration) return;
  if (trim.active) {
    trim.close();
    return;
  }
  playMedia('source', false);
  trim.open(media.duration, clips);
};
translatePage();
library.updateLanguage();
updateWindowTitle();
installPresets();
installCompressionPreview();
button('choose').onclick = button('replace').onclick = () => void chooseVideo();
button('choose-image').onclick = () => void chooseImage();
select('watermark').addEventListener('change', () => {
  const kind = select('watermark').value;
  element('text-fields').hidden = kind !== 'text';
  element('image-fields').hidden = kind !== 'image';
  element('common-fields').hidden = kind === 'off';
});
new ResizeObserver(fitPreview).observe(previewPanel);
layer.addEventListener('pointerdown', event => {
  if (busy || !canStart || !mark || drag || event.button !== 0) return;
  drag = { pointer: event.pointerId, start: { x: event.clientX, y: event.clientY }, origin: { x: mark.x, y: mark.y } };
  layer.setPointerCapture(event.pointerId);
  layer.classList.add('dragging');
});
layer.addEventListener('pointermove', event => {
  if (!frameSize || drag?.pointer !== event.pointerId) return;
  const rect = surface.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  moveMark(
    drag.origin.x + (event.clientX - drag.start.x) * frameSize.width / rect.width,
    drag.origin.y + (event.clientY - drag.start.y) * frameSize.height / rect.height,
  );
});
for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) {
  layer.addEventListener(type, event => {
    if ((event as PointerEvent).pointerId === drag?.pointer) endDrag();
  });
}
layer.addEventListener('keydown', event => {
  if (busy || !canStart || !mark) return;
  const step = event.shiftKey ? 10 : 1;
  switch (event.key) {
    case 'ArrowLeft': moveMark(mark.x - step, mark.y); break;
    case 'ArrowRight': moveMark(mark.x + step, mark.y); break;
    case 'ArrowUp': moveMark(mark.x, mark.y - step); break;
    case 'ArrowDown': moveMark(mark.x, mark.y + step); break;
    default: return;
  }
  event.preventDefault();
});
controls.addEventListener('input', schedulePreview);
button('play-source').onclick = () => playMedia('source');
button('play-result').onclick = () => playMedia('result');
button('still-preview').onclick = showStillPreview;
button('preview-movie').onclick = playOutputPreview;
button('start').onclick = () => void start();
button('cancel').onclick = async () => {
  if (jobId === null) return;
  button('cancel').disabled = true;
  if (await invoke<boolean>('cancel_export', { jobId })) showTask('正在取消', '', null);
};
for (const [id, reveal] of [['open-result', false], ['reveal-result', true]] as const) {
  button(id).onclick = () => void invoke('open_result', { reveal }).catch(error => showTaskDetail(errorFrom(error).message));
}

if ('__TAURI_INTERNALS__' in window) {
  void getCurrentWindow().onDragDropEvent(({ payload }) => {
    const viewer = document.querySelector('.viewer')!;
    viewer.classList.toggle('dragging', !busy && (payload.type === 'enter' || payload.type === 'over'));
    if (payload.type !== 'drop' || busy) return;
    if (payload.paths.length !== 1) {
      message('input-error', '每次请选择一个视频。');
    } else {
      void loadVideo(payload.paths[0]);
    }
  });
  void listen('close-while-running', async () => {
    if (await confirm('关闭视频转码？', '正在转换的任务将取消。已进入写入阶段的任务会完成后关闭。', '关闭应用')) {
      await invoke('exit_after_export');
    }
  });
}
