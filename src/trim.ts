import { bindText, t } from './i18n';

export type Clip = { start: number; end: number };
type Point = 'start' | 'end' | 'playhead';

export const trimMarkup = `
  <section id="trim-timeline" class="trim-timeline" aria-label="截取时间轴" data-i18n-attrs hidden>
    <div class="trim-toolbar">
      <div class="trim-actions">
        <button type="button" class="button subtle" id="trim-play" data-i18n>播放片段</button>
        <button type="button" class="button subtle" id="trim-add" data-i18n>添加片段</button>
        <button type="button" class="button subtle" id="trim-delete" data-i18n>删除片段</button>
      </div>
      <div class="trim-actions">
        <button type="button" class="button subtle" id="trim-reset" data-i18n>恢复全片</button>
        <button type="button" class="button subtle" id="trim-close" data-i18n>取消</button>
        <button type="button" class="button primary" id="trim-apply" data-i18n>确定截取</button>
      </div>
    </div>
    <div class="trim-track-wrap">
      <div id="trim-track" class="trim-track">
        <div id="trim-bands" aria-hidden="true"></div>
        <span class="trim-playhead" aria-hidden="true"></span>
        <div id="trim-start" class="trim-handle" role="slider" tabindex="0"
             aria-label="片段起点" data-i18n-attrs></div>
        <div id="trim-end" class="trim-handle" role="slider" tabindex="0"
             aria-label="片段终点" data-i18n-attrs></div>
      </div>
    </div>
    <div class="trim-footer">
      <div id="trim-clips" class="trim-clips" role="group" aria-label="已选片段" data-i18n-attrs></div>
      <span id="trim-range" class="hint"></span>
    </div>
  </section>`;

const minimumDuration = 0.1;
const maximumClips = 32;

export function totalClipDuration(clips: Clip[]) {
  return clips.reduce((total, { start, end }) => total + end - start, 0);
}

export function installTrimTimeline(
  player: HTMLVideoElement,
  secondsLabel: (seconds: number) => string,
  apply: (clips: Clip[]) => void,
  onToggle: () => void,
) {
  const panel = document.querySelector<HTMLElement>('#trim-timeline')!;
  const track = panel.querySelector<HTMLElement>('#trim-track')!;
  const startHandle = panel.querySelector<HTMLElement>('#trim-start')!;
  const endHandle = panel.querySelector<HTMLElement>('#trim-end')!;
  const range = panel.querySelector<HTMLElement>('#trim-range')!;
  const play = panel.querySelector<HTMLButtonElement>('#trim-play')!;
  const add = panel.querySelector<HTMLButtonElement>('#trim-add')!;
  const remove = panel.querySelector<HTMLButtonElement>('#trim-delete')!;
  const confirm = panel.querySelector<HTMLButtonElement>('#trim-apply')!;
  const bands = panel.querySelector<HTMLElement>('#trim-bands')!;
  const list = panel.querySelector<HTMLElement>('#trim-clips')!;
  const toggle = document.querySelector<HTMLButtonElement>('#trim-button')!;
  let duration = 0;
  let draft: Clip[] | null = null;
  let selected = 0;
  const activeClip = () => draft?.[selected];
  let drag: { pointer: number; point: Point; offset: number } | null = null;
  let pendingSeek: number | null = null;

  function seek(seconds: number) {
    if (!draft) return;
    if (player.readyState === 0) {
      pendingSeek = seconds;
    } else {
      const end = Number.isFinite(player.duration) ? player.duration : duration;
      player.currentTime = Math.min(seconds, end);
      pendingSeek = null;
    }
    updatePlayhead();
  }

  function updatePlayhead() {
    if (!draft) return;
    const position = Math.min(pendingSeek ?? player.currentTime, duration);
    track.style.setProperty('--playhead', `${100 * position / duration}%`);
  }

  function availableGap() {
    if (!draft || draft.length >= maximumClips) return undefined;
    const gaps = [...draft.map((clip, index) => ({
      start: draft![index - 1]?.end ?? 0, end: clip.start, index,
    })), { start: draft.at(-1)?.end ?? 0, end: duration, index: draft.length }]
      .filter(gap => gap.end - gap.start >= minimumDuration - 1e-9);
    return gaps.find(gap => gap.index > selected) ?? gaps[0];
  }

  function selectClip(index: number) {
    if (!draft?.[index] || selected === index) return;
    player.pause();
    selected = index;
    seek(draft[index].start);
    sync();
  }

  function sync() {
    if (!draft) return;
    const clip = activeClip();
    const gap = availableGap();
    add.disabled = !gap;
    add.title = gap ? '' : t(draft.length >= maximumClips ? '最多可选 32 段' : '先拖动把手，留出新片段的位置');
    play.disabled = !clip || player.readyState === 0;
    remove.disabled = confirm.disabled = !clip;
    startHandle.hidden = endHandle.hidden = !clip;

    while (list.childElementCount > draft.length) {
      list.lastElementChild?.remove();
      bands.lastElementChild?.remove();
    }
    for (const [index, { start, end }] of draft.entries()) {
      let chip = list.children[index] as HTMLButtonElement | undefined;
      let band = bands.children[index] as HTMLElement | undefined;
      if (!chip || !band) {
        chip = document.createElement('button');
        chip.type = 'button';
        chip.className = 'trim-chip';
        chip.onclick = () => selectClip(index);
        list.append(chip);
        band = document.createElement('span');
        band.className = 'trim-selection';
        bands.append(band);
      }
      chip.setAttribute('aria-pressed', String(selected === index));
      chip.textContent = `${index + 1}. ${secondsLabel(start)}–${secondsLabel(end)}`;
      chip.setAttribute('aria-label', t('片段 {number}：{start}–{end}', {
        number: index + 1, start: secondsLabel(start), end: secondsLabel(end),
      }));
      band.classList.toggle('selected', selected === index);
      band.style.left = `${100 * start / duration}%`;
      band.style.right = `${100 * (duration - end) / duration}%`;
    }
    const count = draft.length;
    const total = secondsLabel(totalClipDuration(draft));
    bindText(range, !count ? '点击「添加片段」开始截取' : !gap && count === 1 && clip?.start === 0 && clip.end === duration
      ? '拖动把手调整范围，再添加片段'
      : () => t('已选 {count} 段 · 共 {duration}', { count, duration: total }));

    if (clip) {
      const { start, end } = clip;
      track.style.setProperty('--start', `${100 * start / duration}%`);
      track.style.setProperty('--end', `${100 * end / duration}%`);
      for (const [handle, value, min, max] of [
        [startHandle, start, draft[selected - 1]?.end ?? 0, end - minimumDuration],
        [endHandle, end, start + minimumDuration, draft[selected + 1]?.start ?? duration],
      ] as const) {
        handle.setAttribute('aria-valuemin', String(Math.min(min, value)));
        handle.setAttribute('aria-valuemax', String(Math.max(max, value)));
        handle.setAttribute('aria-valuenow', String(value));
        handle.setAttribute('aria-valuetext', secondsLabel(value));
      }
    }
    updatePlayhead();
  }

  function move(point: Point, time: number) {
    if (!draft) return;
    const clip = activeClip();
    player.pause();
    if (clip && point === 'start') {
      clip.start = Math.max(draft[selected - 1]?.end ?? 0, Math.min(time, clip.end - minimumDuration));
      time = clip.start;
    } else if (clip && point === 'end') {
      clip.end = Math.min(draft[selected + 1]?.start ?? duration, Math.max(time, clip.start + minimumDuration));
      time = clip.end;
    }
    seek(time);
    if (point !== 'playhead') sync();
  }

  function dragTo(point: Point, clientX: number) {
    const bounds = track.getBoundingClientRect();
    if (!bounds.width) return;
    const fraction = Math.max(0, Math.min(1, (clientX - bounds.left) / bounds.width));
    const time = fraction === 1 ? duration : Math.round(fraction * duration * 1000) / 1000;
    move(point, Math.min(duration, time));
  }

  track.addEventListener('pointerdown', event => {
    if (!draft || drag || event.button !== 0) return;
    const point = event.target === startHandle ? 'start' : event.target === endHandle ? 'end' : 'playhead';
    const bounds = track.getBoundingClientRect();
    const clip = activeClip();
    const offset = point === 'playhead' || !clip ? 0 : event.clientX - (bounds.left + bounds.width * clip[point] / duration);
    if (point === 'playhead' && bounds.width) {
      const time = (event.clientX - bounds.left) / bounds.width * duration;
      const index = draft.findIndex(({ start, end }) => time >= start && time <= end);
      if (index !== -1) selectClip(index);
    }
    drag = { pointer: event.pointerId, point, offset };
    track.setPointerCapture(event.pointerId);
    if (point === 'playhead') dragTo(point, event.clientX);
    else (point === 'start' ? startHandle : endHandle).focus({ preventScroll: true });
  });
  track.addEventListener('pointermove', event => {
    if (drag?.pointer === event.pointerId) dragTo(drag.point, event.clientX - drag.offset);
  });
  for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) {
    track.addEventListener(type, event => {
      if ((event as PointerEvent).pointerId === drag?.pointer) drag = null;
    });
  }

  for (const [handle, point] of [[startHandle, 'start'], [endHandle, 'end']] as const) {
    handle.addEventListener('keydown', event => {
      const clip = activeClip();
      if (!clip) return;
      const step = event.shiftKey ? 1 : 0.1;
      let next: number;
      switch (event.key) {
        case 'ArrowLeft':
        case 'ArrowDown': next = clip[point] - step; break;
        case 'ArrowRight':
        case 'ArrowUp': next = clip[point] + step; break;
        case 'Home': next = 0; break;
        case 'End': next = duration; break;
        default: return;
      }
      event.preventDefault();
      move(point, next);
    });
  }

  player.addEventListener('loadedmetadata', () => {
    if (draft && pendingSeek !== null) seek(pendingSeek);
    play.disabled = !activeClip();
  });
  player.addEventListener('emptied', () => { play.disabled = true; });
  player.addEventListener('timeupdate', () => {
    if (!draft) return;
    const clip = activeClip();
    if (clip && !player.paused && player.currentTime >= clip.end) {
      player.pause();
      seek(clip.end);
    }
    updatePlayhead();
  });
  for (const event of ['play', 'pause', 'ended']) {
    player.addEventListener(event, () => {
      if (draft) bindText(play, player.paused ? '播放片段' : '暂停');
    });
  }
  play.onclick = () => {
    const clip = activeClip();
    if (!clip || player.readyState === 0) return;
    if (!player.paused) {
      player.pause();
      return;
    }
    if (player.currentTime < clip.start || player.currentTime >= clip.end) seek(clip.start);
    void player.play().catch(() => {});
  };

  function close(restoreFocus = false) {
    if (!draft) return;
    draft = null;
    pendingSeek = null;
    if (drag && track.hasPointerCapture(drag.pointer)) track.releasePointerCapture(drag.pointer);
    drag = null;
    player.pause();
    player.controls = true;
    panel.hidden = true;
    toggle.setAttribute('aria-expanded', 'false');
    onToggle();
    if (restoreFocus) toggle.focus();
  }

  add.onclick = () => {
    const gap = availableGap();
    if (!draft || !gap) return;
    player.pause();
    draft.splice(gap.index, 0, { start: gap.start, end: gap.end });
    selected = gap.index;
    seek(gap.start);
    sync();
    const chip = list.children[selected] as HTMLButtonElement;
    chip.focus({ preventScroll: true });
    chip.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  };
  remove.onclick = () => {
    if (!draft?.length) return;
    player.pause();
    draft.splice(selected, 1);
    selected = Math.max(0, Math.min(selected, draft.length - 1));
    const clip = activeClip();
    if (clip) seek(clip.start);
    sync();
    if (!clip) add.focus();
  };
  confirm.onclick = () => {
    if (!draft?.length) return;
    const [first] = draft;
    const clips = draft.length === 1 && first.start === 0 && first.end === duration ? [] : draft;
    close(true);
    apply(clips);
  };
  panel.querySelector<HTMLButtonElement>('#trim-reset')!.onclick = () => {
    close(true);
    apply([]);
  };
  panel.querySelector<HTMLButtonElement>('#trim-close')!.onclick = () => close(true);
  panel.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      event.preventDefault();
      close(true);
    }
  });

  return {
    get active() { return draft !== null; },
    open(total: number, clips: Clip[]) {
      duration = total;
      draft = clips.length ? clips.map(clip => ({ ...clip })) : [{ start: 0, end: duration }];
      selected = 0;
      panel.hidden = false;
      toggle.setAttribute('aria-expanded', 'true');
      player.pause();
      player.controls = false;
      bindText(play, '播放片段');
      seek(draft[0].start);
      list.scrollLeft = 0;
      sync();
      onToggle();
    },
    close,
    sync,
  };
}
