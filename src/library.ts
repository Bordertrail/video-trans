import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { fileName } from './paths';
import { installWorkspacePreview } from './workspace-preview';

type Preferences = {
  workspaces: string[];
  defaultExportDirectory: string | null;
  exportHistory: string[];
};

type WorkspaceVideo = {
  path: string;
  relativePath: string;
  format: string;
  modifiedMs: number | null;
};

type WorkspaceListing = { videos: WorkspaceVideo[]; skipped: number };

const dialogs = `
  <dialog id="workspace-dialog" class="library-dialog workspace-dialog" aria-labelledby="workspace-title">
    <header class="library-heading">
      <h2 id="workspace-title">工作空间</h2>
      <form method="dialog"><button class="button subtle">关闭</button></form>
    </header>
    <div class="workspace-tools">
      <label class="field">
        文件夹
        <select id="workspace-folder"><option value="">请选择工作空间</option></select>
      </label>
      <div class="library-actions">
        <button id="add-workspace" class="button secondary" type="button">添加文件夹</button>
        <button id="refresh-workspace" class="button subtle" type="button" hidden>刷新</button>
        <button id="remove-workspace" class="button subtle" type="button" hidden>移除</button>
      </div>
    </div>
    <div id="workspace-filters" class="workspace-filters" hidden>
      <label class="field">修改时间
        <select id="workspace-period">
          <option value="all">全部时间</option><option value="7">最近 7 天</option>
          <option value="30">最近 30 天</option><option value="90">最近 90 天</option>
          <option value="custom">自定义日期</option>
        </select>
      </label>
      <label class="field">视频类型
        <select id="workspace-format"><option value="">全部类型</option></select>
      </label>
      <label class="field">分组
        <select id="workspace-group">
          <option value="date">按修改日期</option><option value="format">按视频类型</option>
          <option value="none">不分组</option>
        </select>
      </label>
      <label class="field">排序
        <select id="workspace-sort"><option value="newest">最近修改优先</option><option value="oldest">最早修改优先</option></select>
      </label>
    </div>
    <div id="workspace-dates" class="split workspace-dates" hidden>
      <label class="field">起始日期 <input id="workspace-from" type="date"></label>
      <label class="field">结束日期 <input id="workspace-to" type="date"></label>
    </div>
    <p id="workspace-status" class="hint library-status" role="status"></p>
    <div id="workspace-videos" class="library-list" aria-label="工作空间视频"></div>
  </dialog>
  <dialog id="export-directory-dialog" class="library-dialog export-directory-dialog" aria-labelledby="export-directory-title">
    <header class="library-heading">
      <h2 id="export-directory-title">导出目录</h2>
      <form method="dialog"><button class="button subtle">关闭</button></form>
    </header>
    <div class="directory-section">
      <h3>默认导出目录</h3>
      <p id="export-default-path"></p>
      <div class="library-actions">
        <button id="choose-export-directory" class="button secondary" type="button">选择文件夹</button>
        <button id="reset-export-directory" class="button subtle" type="button" hidden>使用源视频目录</button>
      </div>
    </div>
    <div id="temporary-directory" class="directory-section" hidden>
      <h3>下次导出目录</h3>
      <p id="temporary-directory-path"></p>
      <button id="reset-temporary-directory" class="button subtle" type="button">取消本次选择</button>
    </div>
    <h3>历史目录</h3>
    <p id="export-directory-status" class="hint library-status" role="status"></p>
    <div id="export-directory-history" class="library-list" aria-label="历史导出目录"></div>
  </dialog>`;

const pageSize = 100;
const day = new Intl.DateTimeFormat('zh-CN', { year: 'numeric', month: 'long', day: 'numeric' });
const get = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const select = (id: string) => get<HTMLSelectElement>(id);
const button = (id: string) => get<HTMLButtonElement>(id);
const input = (id: string) => get<HTMLInputElement>(id);

function makeButton(label: string, style = 'secondary') {
  const control = document.createElement('button');
  control.type = 'button';
  control.className = `button ${style}`;
  control.textContent = label;
  return control;
}

function status(id: string, text: string, error = false) {
  const node = get(id);
  node.textContent = text;
  node.classList.toggle('error', error);
  node.hidden = !text;
}

export function installLibrary(
  loadVideo: (path: string) => Promise<void>,
  errorMessage: (reason: unknown) => string,
  pauseMainPlayback: () => (() => void) | undefined,
) {
  document.body.insertAdjacentHTML('beforeend', dialogs);
  const showPreview = installWorkspacePreview(errorMessage, pauseMainPlayback);
  const workspaceDialog = get<HTMLDialogElement>('workspace-dialog');
  const exportDialog = get<HTMLDialogElement>('export-directory-dialog');
  const folder = select('workspace-folder');
  const format = select('workspace-format');
  const workspaceList = get('workspace-videos');
  const historyList = get('export-directory-history');

  let preferences: Preferences = { workspaces: [], defaultExportDirectory: null, exportHistory: [] };
  let selectedWorkspace = '';
  let listing: WorkspaceListing | null = null;
  let scanId = 0;
  let visible = pageSize;
  let temporaryDirectory: string | null = null;

  function showWorkspaces() {
    folder.replaceChildren(new Option('请选择工作空间', ''));
    for (const path of preferences.workspaces) folder.add(new Option(path, path));
    selectedWorkspace = preferences.workspaces.includes(selectedWorkspace)
      ? selectedWorkspace : preferences.workspaces[0] ?? '';
    folder.value = selectedWorkspace;
    button('refresh-workspace').hidden = !selectedWorkspace;
    button('remove-workspace').hidden = !selectedWorkspace;
  }

  function showDirectoryPreferences() {
    const preferred = preferences.defaultExportDirectory;
    const summary = get('export-directory-summary');
    summary.textContent = temporaryDirectory
      ? `本次使用 ${fileName(temporaryDirectory)}` : preferred ? fileName(preferred) : '源视频目录';
    summary.title = temporaryDirectory ?? preferred ?? '';
    get('export-default-path').textContent = preferred ?? '源视频目录';
    button('reset-export-directory').hidden = !preferred;
    get('temporary-directory').hidden = !temporaryDirectory;
    get('temporary-directory-path').textContent = temporaryDirectory;
    historyList.replaceChildren();
    for (const path of preferences.exportHistory) {
      const row = document.createElement('div');
      row.className = 'library-row';
      const label = document.createElement('span');
      label.textContent = path;
      label.title = path;
      const choose = makeButton('本次使用');
      choose.onclick = async () => {
        try {
          const directory = await invoke<string>('export_directory', { path });
          temporaryDirectory = directory;
          showDirectoryPreferences();
          exportDialog.close();
        } catch (reason) {
          status('export-directory-status', errorMessage(reason), true);
        }
      };
      row.append(label, choose);
      historyList.append(row);
    }
    status('export-directory-status', preferences.exportHistory.length ? '' : '开始转换后，所选目录会显示在这里。');
  }

  function renderVideos() {
    if (!listing) return;
    workspaceList.replaceChildren();
    const period = select('workspace-period').value;
    const lower = period === 'custom' ? input('workspace-from').valueAsNumber : NaN;
    const upper = period === 'custom' ? input('workspace-to').valueAsNumber : NaN;
    const from = Number.isFinite(lower) ? localDate(lower) : null;
    const until = Number.isFinite(upper) ? localDate(upper) : null;
    if (from && until && from > until) {
      status('workspace-status', '起始日期应早于或等于结束日期。', true);
      return;
    }
    until?.setDate(until.getDate() + 1);
    const recentDays = Number(period);
    const recent = new Date();
    if (Number.isFinite(recentDays)) recent.setDate(recent.getDate() - recentDays);
    const minTime = Number.isFinite(recentDays) ? recent.getTime() : from?.getTime() ?? null;
    const maxTime = until?.getTime() ?? null;
    const type = format.value;
    const newest = select('workspace-sort').value === 'newest';
    const groupBy = select('workspace-group').value;
    const videos = listing.videos.filter(video => {
      const modified = video.modifiedMs;
      return (!type || video.format === type)
        && (minTime === null || (modified !== null && modified >= minTime))
        && (maxTime === null || (modified !== null && modified < maxTime));
    }).sort((a, b) => {
      if (a.modifiedMs === null || b.modifiedMs === null) {
        return Number(a.modifiedMs === null) - Number(b.modifiedMs === null) || a.relativePath.localeCompare(b.relativePath);
      }
      return (newest ? b.modifiedMs - a.modifiedMs : a.modifiedMs - b.modifiedMs)
        || a.relativePath.localeCompare(b.relativePath);
    });

    const skipped = listing.skipped ? `，跳过 ${listing.skipped} 个无法读取的条目` : '';
    status('workspace-status', `${videos.length ? `${videos.length} 个视频` : '没有符合条件的视频'}${skipped}`);
    const groups = new Map<string, WorkspaceVideo[]>();
    for (const video of videos) {
      const date = video.modifiedMs === null ? '修改日期未知' : day.format(video.modifiedMs);
      const group = groupBy === 'none' ? '' : groupBy === 'format' ? video.format.toUpperCase() : date;
      const entries = groups.get(group);
      if (entries) entries.push(video);
      else groups.set(group, [video]);
    }

    let remaining = visible;
    for (const [group, entries] of groups) {
      if (!remaining) break;
      if (group) {
        const heading = document.createElement('h3');
        heading.textContent = `${group} · ${entries.length}`;
        workspaceList.append(heading);
      }
      for (const video of entries.slice(0, remaining)) {
        const row = document.createElement('div');
        row.className = 'library-row';
        const summary = document.createElement('div');
        const name = document.createElement('strong');
        name.textContent = video.relativePath;
        name.title = video.relativePath;
        const info = document.createElement('span');
        info.textContent = `${video.format.toUpperCase()} · ${video.modifiedMs === null ? '修改日期未知' : day.format(video.modifiedMs)}`;
        const importVideo = () => {
          workspaceDialog.close();
          void loadVideo(video.path);
        };
        const preview = makeButton('预览', 'subtle');
        preview.setAttribute('aria-label', `预览 ${video.relativePath}`);
        const workspace = selectedWorkspace;
        preview.onclick = () => showPreview({
          workspace, path: video.path, name: fileName(video.relativePath),
          detail: info.textContent, importVideo,
        });
        const add = makeButton('导入');
        add.onclick = importVideo;
        const actions = document.createElement('div');
        actions.className = 'library-actions';
        actions.append(preview, add);
        summary.append(name, info);
        row.append(summary, actions);
        workspaceList.append(row);
      }
      remaining -= Math.min(remaining, entries.length);
    }
    if (videos.length > visible) {
      const more = makeButton('显示更多');
      more.classList.add('library-more');
      more.onclick = () => {
        const scroll = workspaceList.scrollTop;
        visible += pageSize;
        renderVideos();
        workspaceList.scrollTop = scroll;
        workspaceList.querySelector<HTMLButtonElement>('.library-more')?.focus({ preventScroll: true });
      };
      workspaceList.append(more);
    }
  }

  async function scanWorkspace() {
    const id = ++scanId;
    const path = selectedWorkspace;
    listing = null;
    workspaceList.replaceChildren();
    get('workspace-filters').hidden = true;
    get('workspace-dates').hidden = true;
    button('refresh-workspace').disabled = true;
    if (!path) {
      status('workspace-status', '添加一个文件夹后，可以直接导入其中的视频。');
      return;
    }
    status('workspace-status', '正在读取视频…');
    try {
      const current = await invoke<WorkspaceListing>('workspace_videos', { path });
      if (id !== scanId) return;
      listing = current;
      const previousFormat = format.value;
      format.replaceChildren(new Option('全部类型', ''));
      for (const value of [...new Set(current.videos.map(video => video.format))].sort()) {
        format.add(new Option(value.toUpperCase(), value));
      }
      format.value = Array.from(format.options).some(option => option.value === previousFormat) ? previousFormat : '';
      get('workspace-filters').hidden = false;
      get('workspace-dates').hidden = select('workspace-period').value !== 'custom';
      visible = pageSize;
      renderVideos();
    } catch (reason) {
      if (id === scanId) status('workspace-status', errorMessage(reason), true);
    } finally {
      if (id === scanId) button('refresh-workspace').disabled = false;
    }
  }

  async function refreshPreferences() {
    preferences = await invoke<Preferences>('library_preferences');
  }

  button('workspaces').onclick = async () => {
    workspaceDialog.showModal();
    try {
      await refreshPreferences();
      showWorkspaces();
      await scanWorkspace();
    } catch (reason) {
      status('workspace-status', errorMessage(reason), true);
    }
  };

  async function updateWorkspace(add: boolean) {
    const buttons = [button('add-workspace'), button('remove-workspace')];
    buttons.forEach(control => { control.disabled = true; });
    folder.disabled = true;
    try {
      const path = add
        ? await open({ title: '添加工作空间文件夹', directory: true, multiple: false })
        : selectedWorkspace;
      if (typeof path !== 'string' || !path) return;
      if (add) {
        const result = await invoke<{ path: string; preferences: Preferences }>('add_workspace', { path });
        selectedWorkspace = result.path;
        preferences = result.preferences;
      } else {
        preferences = await invoke<Preferences>('remove_workspace', { path });
      }
      showWorkspaces();
      await scanWorkspace();
    } catch (reason) {
      status('workspace-status', errorMessage(reason), true);
    } finally {
      buttons.forEach(control => { control.disabled = false; });
      folder.disabled = false;
    }
  }

  button('add-workspace').onclick = () => void updateWorkspace(true);
  button('remove-workspace').onclick = () => void updateWorkspace(false);
  button('refresh-workspace').onclick = () => void scanWorkspace();
  folder.onchange = () => {
    selectedWorkspace = folder.value;
    button('refresh-workspace').hidden = !selectedWorkspace;
    button('remove-workspace').hidden = !selectedWorkspace;
    void scanWorkspace();
  };
  const filter = () => {
    get('workspace-dates').hidden = select('workspace-period').value !== 'custom';
    visible = pageSize;
    workspaceList.scrollTop = 0;
    renderVideos();
  };
  get('workspace-filters').addEventListener('change', filter);
  get('workspace-dates').addEventListener('change', filter);

  button('output-dirs').onclick = async () => {
    exportDialog.showModal();
    try {
      await refreshPreferences();
      showDirectoryPreferences();
    } catch (reason) {
      status('export-directory-status', errorMessage(reason), true);
    }
  };

  async function updateDefault(selectFolder: boolean) {
    const buttons = [button('choose-export-directory'), button('reset-export-directory')];
    buttons.forEach(control => { control.disabled = true; });
    try {
      const path = selectFolder ? await open({ title: '选择默认导出目录', directory: true, multiple: false }) : null;
      if (selectFolder && typeof path !== 'string') return;
      preferences = await invoke<Preferences>('set_export_directory', { path });
      temporaryDirectory = null;
      showDirectoryPreferences();
    } catch (reason) {
      status('export-directory-status', errorMessage(reason), true);
    } finally {
      buttons.forEach(control => { control.disabled = false; });
    }
  }

  button('choose-export-directory').onclick = () => void updateDefault(true);
  button('reset-export-directory').onclick = () => void updateDefault(false);
  button('reset-temporary-directory').onclick = () => {
    temporaryDirectory = null;
    showDirectoryPreferences();
  };

  if ('__TAURI_INTERNALS__' in window) {
    void refreshPreferences().then(showDirectoryPreferences).catch(() => {});
  }

  return {
    async exportDirectory(): Promise<string | null> {
      return invoke<string | null>('export_directory', { path: temporaryDirectory });
    },
    exportStarted() {
      temporaryDirectory = null;
      showDirectoryPreferences();
    },
  };
}

function localDate(utc: number) {
  const date = new Date(utc);
  return new Date(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate());
}
