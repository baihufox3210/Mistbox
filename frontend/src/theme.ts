// Appearance: light/dark theme, layout, background image and the
// admin-editable /theme/custom.css. Per-device prefs live in localStorage
// (no credentials are ever stored there); the background is a server setting.
import type { UiSettings } from './api';

export type ThemePref = 'light' | 'dark' | 'system';
export type LogoPref = 'auto' | 'dark' | 'light';
export type Layout = 'drive' | 'ide';

const KEY_THEME = 'mistbox.theme';
const KEY_LOGO = 'mistbox.logo.v2';
const KEY_LAYOUT = 'mistbox.layout';

export function getThemePref(): ThemePref {
  const v = localStorage.getItem(KEY_THEME);
  return v === 'light' || v === 'dark' ? v : 'system';
}

export function getLogoPref(): LogoPref {
  const pref = localStorage.getItem(KEY_LOGO);
  return pref === 'light' || pref === 'dark' ? pref : 'auto';
}

export function applyLogo(pref: LogoPref = getLogoPref()) {
  localStorage.setItem(KEY_LOGO, pref);
  const color = pref === 'auto' ? resolvedTheme() : pref;
  document.documentElement.dataset.logo = color;
  const icon = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (icon) icon.href = `/logo-${color}.svg`;
}

export function getLayout(): Layout {
  const v = localStorage.getItem(KEY_LAYOUT);
  if (v === 'drive' || v === 'ide') return v;
  return 'drive';
}

const media = () => window.matchMedia?.('(prefers-color-scheme: dark)');

export function resolvedTheme(pref = getThemePref()): 'light' | 'dark' {
  if (pref === 'system') return media()?.matches ? 'dark' : 'light';
  return pref;
}

export function applyTheme(pref: ThemePref = getThemePref()) {
  localStorage.setItem(KEY_THEME, pref);
  document.documentElement.dataset.theme = resolvedTheme(pref);
  document.documentElement.dataset.themePref = pref;
  applyLogo();
  window.dispatchEvent(new Event('mistbox:theme'));
}

export function setLayout(l: Layout) {
  localStorage.setItem(KEY_LAYOUT, l);
  document.documentElement.dataset.layout = l;
}

/** Follow OS changes while in "system" mode. */
export function initTheme() {
  applyTheme(getThemePref());
  document.documentElement.dataset.layout = getLayout();
  media()?.addEventListener('change', () => getThemePref() === 'system' && applyTheme('system'));
}

export function applyBackground(s: Pick<UiSettings, 'background_url' | 'background_dim' | 'background_blur'>) {
  const root = document.documentElement;
  if (s.background_url) {
    // The server only accepts URLs without quotes / parens / spaces, so this cannot break out of url("…").
    root.style.setProperty('--mb-bg-image', `url("${s.background_url}")`);
    root.style.setProperty('--mb-bg-dim', String(s.background_dim));
    root.style.setProperty('--mb-bg-blur', `${s.background_blur}px`);
    root.dataset.hasBg = 'true';
  } else {
    root.style.removeProperty('--mb-bg-image');
    root.style.removeProperty('--mb-bg-dim');
    root.style.removeProperty('--mb-bg-blur');
    delete root.dataset.hasBg;
  }
}

const CUSTOM_ID = 'mistbox-custom-css';

/**
 * Load /theme/custom.css as the *last* stylesheet so it overrides everything,
 * including CSS of lazily loaded chunks (kept last via a MutationObserver).
 */
export function loadCustomCss(bust = false) {
  let link = document.getElementById(CUSTOM_ID) as HTMLLinkElement | null;
  if (!link) {
    link = document.createElement('link');
    link.id = CUSTOM_ID;
    link.rel = 'stylesheet';
    document.head.appendChild(link);
    new MutationObserver(() => {
      const l = document.getElementById(CUSTOM_ID);
      if (l && l !== document.head.lastElementChild) {
        const last = document.head.lastElementChild;
        if (last && (last.tagName === 'STYLE' || last.tagName === 'LINK')) document.head.appendChild(l);
      }
    }).observe(document.head, { childList: true });
  }
  link.href = `/theme/custom.css${bust ? `?v=${Date.now()}` : ''}`;
}
