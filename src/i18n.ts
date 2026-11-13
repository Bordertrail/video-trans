import { english } from './english';

export type Language = 'zh-CN' | 'en';
type Text = string | (() => string);

const preference = 'video-trans-language';
let language: Language = 'zh-CN';
try {
  if (localStorage.getItem(preference) === 'en') language = 'en';
} catch {
  // Language switching also works when storage is unavailable.
}

const textBindings = new WeakMap<HTMLElement, Text>();
const attributes = ['aria-label', 'title', 'placeholder', 'alt'];
const attributeBindings = new WeakMap<HTMLElement, Map<string, string>>();

export function locale(): Language {
  return language;
}

export function t(text: string, values: Record<string, string | number> = {}): string {
  if (language === 'en' && !Object.hasOwn(english, text)) {
    const extension = /^文件扩展名应为 \.([a-z0-9]+)。$/.exec(text);
    if (extension) return t('文件扩展名应为 .{ext}。', { ext: extension[1] });
  }
  const translated = language === 'en' && Object.hasOwn(english, text) ? english[text] : text;
  return translated.replace(/\{(\w+)\}/g, (match, name: string) =>
    Object.hasOwn(values, name) ? String(values[name]) : match);
}

export function bindText(node: HTMLElement, text: Text) {
  node.setAttribute('data-i18n', '');
  textBindings.set(node, text);
  node.textContent = typeof text === 'function' ? text() : t(text);
}

export function bindAttribute(node: HTMLElement, attribute: string, text: string) {
  const bindings = attributeBindings.get(node) ?? new Map();
  bindings.set(attribute, text);
  attributeBindings.set(node, bindings);
  node.setAttribute('data-i18n-attrs', '');
  node.setAttribute(attribute, t(text));
}

export function translatePage() {
  document.documentElement.lang = language;
  document.title = t('视频转码');
  document.querySelectorAll<HTMLElement>('[data-i18n]').forEach(node => {
    bindText(node, textBindings.get(node) ?? node.textContent ?? '');
  });
  document.querySelectorAll<HTMLElement>('[data-i18n-attrs]').forEach(node => {
    const bindings = attributeBindings.get(node);
    for (const attribute of attributes) {
      const text = bindings ? bindings.get(attribute) : node.getAttribute(attribute);
      if (text != null) bindAttribute(node, attribute, text);
    }
  });
}

export function setLanguage(next: Language) {
  language = next;
  try {
    localStorage.setItem(preference, language);
  } catch {
    // Keep the selection for the current session.
  }
  translatePage();
}
