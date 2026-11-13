import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { bindText, locale, t } from './i18n';
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
      <h2 id="workspace-title" data-i18n>工作空间</h2>
      <form method="dialog"><button class="button subtle" data-i18n>关闭</button></form>
    </header>
    <div class="workspace-tools">
      <label class="field">
        <span data-i18n>文件夹</span>
        <select id="workspace-folder"><option value="" data-i18n>请选择工作空间</option></select>
      </label>
      <div class="library-actions">
        <button id="add-workspace" class="button secondary" type="button" data-i18n>添加文件夹</button>
        <button id="refresh-workspace" class="button subtle" type="button" hidden data-i18n>刷新</button>
        <button id="remove-workspace" class="button subtle" type="button" hidden data-i18n>移除</button>
      </div>
    </div>
    <div id="workspace-filters" class="workspace-filters" hidden>
      <label class="field"><span data-i18n>修改时间</span>
        <select id="workspace-period">
          <option value="all" data-i18n>全部时间</option><option value="7" data-i18n>最近 7 天</option>
          <option value="30" data-i18n>最近 30 天</option><option value="90" data-i18n>最近 90 天</option>
          <option value="custom" data-i18n>自定义日期</option>
        </select>
      </label>
      <label class="field"><span data-i18n>视频类型</span>
        <select id="workspace-format"><option value="" data-i18n>全部类型</option></select>
      </label>
      <label class="field"><span data-i18n>分组</span>
        <select id="workspace-group">
          <option value="date" data-i18n>按修改日期</option><option value="format" data-i18n>按视频类型</option>
          <option value="none" data-i18n>不分组</option>
        </select>
      </label>
      <label class="field"><span data-i18n>排序</span>
        <select id="workspace-sort"><option value="newest" data-i18n>最近修改优先</option><option value="oldest" data-i18n>最早修改优先</option></select>
      </label>
    </div>
    <div id="workspace-dates" class="split workspace-dates" hidden>
      <label class="field"><span data-i18n>起始日期</span> <input id="workspace-from" type="date"></label>
      <label class="field"><span data-i18n>结束日期</span> <input id="workspace-to" type="date"></label>
    </div>
    <p id="workspace-status" class="hint library-status" role="status"></p>
    <div id="workspace-videos" class="library-list" aria-label="工作空间视频" data-i18n-attrs></div>
  </dialog>
  <dialog id="export-directory-dialog" class="library-dialog export-directory-dialog" aria-labelledby="export-directory-title">
    <header class="library-heading">
      <h2 id="export-directory-title" data-i18n>导出目录</h2>
      <form method="dialog"><button class="button subtle" data-i18n>关闭</button></form>
    </header>
    <div class="directory-section">
      <h3 data-i18n>默认导出目录</h3>
      <p id="export-default-path"></p>
      <div class="library-actions">
        <button id="choose-export-directory" class="button secondary" type="button" data-i18n>选择文件夹</button>
        <button id="reset-export-directory" class="button subtle" type="button" hidden data-i18n>使用源视频目录</button>
      </div>
    </div>
    <div id="temporary-directory" class="directory-section" hidden>
      <h3 data-i18n>下次导出目录</h3>
      <p id="temporary-directory-path"></p>
      <button id="reset-temporary-directory" class="button subtle" type="button" data-i18n>取消本次选择</button>
    </div>
    <h3 data-i18n>历史目录</h3>
    <p id="export-directory-status" class="hint library-status" role="status"></p>
    <div id="export-directory-history" class="library-list" aria-label="历史导出目录" data-i18n-attrs></div>
  </dialog>`;

const pageSize = 100;
function modifiedDate(timestamp: number | null) {
  return timestamp === null ? t('修改日期未知') : new Intl.DateTimeFormat(locale(), {
    year: 'numeric', month: 'long', day: 'numeric',
  }).format(timestamp);
}
const get = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const select = (id: string) => get<HTMLSelectElement>(id);
const button = (id: string) => get<HTMLButtonElement>(id);
const input = (id: string) => get<HTMLInputElement>(id);

function makeButton(label: string, style = 'secondary') {
  const control = document.createElement('button');
  control.type = 'button';
  control.className = `button ${style}`;
  bindText(control, label);
  return control;
}

function status(id: string, text: string | (() => string), error = false) {
  const node = get(id);
  bindText(node, text);
  node.classList.toggle('error', error);
  node.hidden = !node.textContent;
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
  let editingDirectories = false;

  function showWorkspaces() {
    folder.replaceChildren(new Option(t('请选择工作空间'), ''));
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
      ? t('本次使用 {folder}', { folder: fileName(temporaryDirectory) }) : preferred ? fileName(preferred) : t('源视频目录');
    summary.title = temporaryDirectory ?? preferred ?? '';
    get('export-default-path').textContent = preferred ?? t('源视频目录');
    button('reset-export-directory').hidden = !preferred;
    get('temporary-directory').hidden = !temporaryDirectory;
    get('temporary-directory-path').textContent = temporaryDirectory;
    const scroll = historyList.scrollTop;
    historyList.replaceChildren();
    for (const path of preferences.exportHistory) {
      const row = document.createElement('div');
      row.className = 'library-row';
      const label = document.createElement('span');
      label.textContent = path;
      label.title = path;
      const choose = makeButton('本次使用');
      choose.onclick = () => void editDirectories(async () => {
        temporaryDirectory = await invoke<string>('export_directory', { path });
        showDirectoryPreferences();
        exportDialog.close();
      });
      const remove = makeButton('移除', 'subtle');
      remove.classList.add('remove-export-history');
      remove.setAttribute('aria-label', t('移除历史记录：{path}', { path }));
      remove.onclick = () => void editDirectories(async () => {
        const index = preferences.exportHistory.indexOf(path);
        preferences = await invoke<Preferences>('remove_export_history', { path });
        if (temporaryDirectory === path) temporaryDirectory = null;
        showDirectoryPreferences();
        if (exportDialog.open) {
          const buttons = historyList.querySelectorAll<HTMLButtonElement>('.remove-export-history');
          const next = buttons[Math.min(index, buttons.length - 1)] ?? button('choose-export-directory');
          next.focus({ preventScroll: true });
        }
      });
      const actions = document.createElement('div');
      actions.className = 'library-actions';
      actions.append(choose, remove);
      row.append(label, actions);
      historyList.append(row);
    }
    historyList.scrollTop = scroll;
    status('export-directory-status', preferences.exportHistory.length
      ? '移除仅清除历史记录。' : '开始转换后，所选目录会显示在这里。');
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

    const skipped = listing.skipped;
    status('workspace-status', () => {
      const count = videos.length ? t('{count} 个视频', { count: videos.length }) : t('没有符合条件的视频');
      return skipped ? count + t('，跳过 {count} 个无法读取的条目', { count: skipped }) : count;
    });
    const groups = new Map<string, WorkspaceVideo[]>();
    for (const video of videos) {
      const date = modifiedDate(video.modifiedMs);
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
        info.textContent = `${video.format.toUpperCase()} · ${modifiedDate(video.modifiedMs)}`;
        const importVideo = () => {
          workspaceDialog.close();
          void loadVideo(video.path);
        };
        const preview = makeButton('预览', 'subtle');
        preview.setAttribute('aria-label', t('预览 {path}', { path: video.relativePath }));
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
      format.replaceChildren(new Option(t('全部类型'), ''));
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
        ? await open({ title: t('添加工作空间文件夹'), directory: true, multiple: false })
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
    if (editingDirectories) return;
    exportDialog.showModal();
    await editDirectories(async () => {
      await refreshPreferences();
      showDirectoryPreferences();
    });
  };

  async function editDirectories(edit: () => Promise<void>) {
    if (editingDirectories) return;
    editingDirectories = true;
    try {
      await edit();
    } catch (reason) {
      status('export-directory-status', errorMessage(reason), true);
    } finally {
      editingDirectories = false;
    }
  }

  function updateDefault(selectFolder: boolean) {
    return editDirectories(async () => {
      const path = selectFolder ? await open({ title: t('选择默认导出目录'), directory: true, multiple: false }) : null;
      if (selectFolder && typeof path !== 'string') return;
      preferences = await invoke<Preferences>('set_export_directory', { path });
      temporaryDirectory = null;
      showDirectoryPreferences();
    });
  }

  button('choose-export-directory').onclick = () => void updateDefault(true);
  button('reset-export-directory').onclick = () => void updateDefault(false);
  button('reset-temporary-directory').onclick = () => {
    if (editingDirectories) return;
    temporaryDirectory = null;
    showDirectoryPreferences();
  };

  if ('__TAURI_INTERNALS__' in window) {
    void refreshPreferences().then(showDirectoryPreferences).catch(() => {});
  }

  return {
    updateLanguage() {
      showWorkspaces();
      const allFormats = format.options[0];
      if (allFormats?.value === '') allFormats.text = t('全部类型');
      renderVideos();
      showDirectoryPreferences();
    },
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
