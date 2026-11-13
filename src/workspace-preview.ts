import { invoke } from '@tauri-apps/api/core';
import { bindText } from './i18n';

type PreviewVideo = {
  workspace: string;
  path: string;
  name: string;
  detail: string;
  importVideo: () => void;
};

const markup = `
  <dialog id="workspace-preview" class="workspace-preview" aria-labelledby="workspace-preview-title">
    <header class="workspace-preview-header">
      <div>
        <h2 id="workspace-preview-title" class="truncate"></h2>
        <p id="workspace-preview-detail"></p>
      </div>
      <button id="workspace-preview-close" type="button" aria-label="关闭视频预览" class="button subtle" autofocus data-i18n-attrs>×</button>
    </header>
    <div class="workspace-preview-stage">
      <video id="workspace-preview-player" controls playsinline preload="metadata" aria-label="视频预览播放器" data-i18n-attrs></video>
      <div id="workspace-preview-status" class="workspace-preview-status" role="status">
        <span></span>
      </div>
    </div>
    <footer class="workspace-preview-footer">
      <button id="workspace-preview-import" type="button" class="button primary" data-i18n>导入视频</button>
    </footer>
  </dialog>`;

export function installWorkspacePreview(
  errorMessage: (reason: unknown) => string,
  pauseMainPlayback: () => (() => void) | undefined,
) {
  document.body.insertAdjacentHTML('beforeend', markup);
  const dialog = document.querySelector<HTMLDialogElement>('#workspace-preview')!;
  const title = dialog.querySelector<HTMLElement>('#workspace-preview-title')!;
  const detail = dialog.querySelector<HTMLElement>('#workspace-preview-detail')!;
  const player = dialog.querySelector<HTMLVideoElement>('video')!;
  const status = dialog.querySelector<HTMLElement>('#workspace-preview-status')!;
  const statusText = status.querySelector('span')!;
  const closeButton = dialog.querySelector<HTMLButtonElement>('#workspace-preview-close')!;
  const importButton = dialog.querySelector<HTMLButtonElement>('#workspace-preview-import')!;
  const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
  let current: PreviewVideo | null = null;
  let requestId = 0;
  let closing = false;
  let resumeMainPlayback: (() => void) | undefined;

  function showStatus(message: string, loading = false) {
    bindText(statusText, message);
    status.classList.toggle('is-loading', loading);
    status.hidden = !message;
  }

  function resetPlayer() {
    player.onloadedmetadata = null;
    player.onerror = null;
    player.pause();
    player.removeAttribute('src');
    player.load();
    player.classList.remove('is-visible');
  }

  async function load(video: PreviewVideo, id: number, compatible: boolean) {
    resetPlayer();
    showStatus(compatible ? '正在准备兼容预览…' : '正在打开视频…', true);
    try {
      const url = await invoke<string | null>('workspace_playback_url', {
        workspace: video.workspace, path: video.path, requestId: id, compatible,
      });
      if (id !== requestId) return;
      if (!url) {
        if (compatible) showStatus('无法预览此视频。');
        else void load(video, id, true);
        return;
      }
      player.onloadedmetadata = () => {
        if (id !== requestId) return;
        player.classList.add('is-visible');
        showStatus('');
      };
      player.onerror = () => {
        if (id !== requestId) return;
        if (!compatible) {
          void load(video, id, true);
        } else {
          resetPlayer();
          showStatus('无法播放此视频。');
        }
      };
      player.src = url;
      void player.play().catch(() => {});
    } catch (reason) {
      if (id === requestId) showStatus(errorMessage(reason));
    }
  }

  async function close(importVideo = false) {
    if (!dialog.open || closing) return;
    const selected = current;
    closing = true;
    current = null;
    const id = ++requestId;
    player.pause();
    void invoke('stop_workspace_playback', { requestId: id }).catch(() => {});
    let exitAnimation: Animation | undefined;
    if (!reducedMotion.matches) {
      const { opacity, transform } = getComputedStyle(dialog);
      dialog.classList.add('is-closing');
      exitAnimation = dialog.animate(
        [{ opacity, transform }, { opacity: 0, transform: 'translateY(8px) scale(.985)' }],
        { duration: 150, easing: 'cubic-bezier(.4, 0, 1, 1)', fill: 'forwards' },
      );
      await exitAnimation.finished.catch(() => {});
    }
    dialog.close();
    exitAnimation?.cancel();
    dialog.classList.remove('is-closing');
    resetPlayer();
    closing = false;
    const resume = resumeMainPlayback;
    resumeMainPlayback = undefined;
    if (importVideo) selected?.importVideo();
    else resume?.();
  }

  closeButton.onclick = () => void close();
  importButton.onclick = () => void close(true);
  dialog.addEventListener('cancel', event => {
    event.preventDefault();
    void close();
  });
  dialog.addEventListener('click', event => {
    if (event.target !== dialog) return;
    const box = dialog.getBoundingClientRect();
    if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) {
      void close();
    }
  });

  return (video: PreviewVideo) => {
    if (dialog.open) return;
    current = video;
    resumeMainPlayback = pauseMainPlayback();
    title.textContent = video.name;
    title.title = video.name;
    detail.textContent = video.detail;
    dialog.showModal();
    void load(video, ++requestId, false);
  };
}
