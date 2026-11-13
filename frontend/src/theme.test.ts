import { describe, expect, it } from 'vitest';
import { applyBackground, applyLogo, applyTheme, getLogoPref, loadCustomCss, resolvedTheme } from './theme';

describe('appearance', () => {
  it('remembers an independent logo color and updates the tab icon', () => {
    const icon = document.createElement('link');
    icon.rel = 'icon';
    document.head.appendChild(icon);
    applyTheme('dark');
    applyLogo('light');
    expect(getLogoPref()).toBe('light');
    expect(document.documentElement.dataset.logo).toBe('light');
    expect(icon.getAttribute('href')).toBe('/logo-light.svg');
    applyTheme('light');
    expect(document.documentElement.dataset.logo).toBe('light');
    applyLogo('dark');
    expect(icon.getAttribute('href')).toBe('/logo-dark.svg');
    icon.remove();
  });

  it('applies explicit themes and remembers the preference', () => {
    applyTheme('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(localStorage.getItem('mistbox.theme')).toBe('dark');
    applyTheme('light');
    expect(resolvedTheme()).toBe('light');
  });

  it('sets and clears the background variables', () => {
    applyBackground({ background_url: '/theme/bg.jpg', background_dim: 0.5, background_blur: 4 });
    const s = document.documentElement.style;
    expect(s.getPropertyValue('--mb-bg-image')).toBe('url("/theme/bg.jpg")');
    expect(s.getPropertyValue('--mb-bg-blur')).toBe('4px');
    expect(s.getPropertyValue('--mb-bg-dim')).toBe('0.5');
    expect(document.documentElement.dataset.hasBg).toBe('true');
    applyBackground({ background_url: '', background_dim: 0, background_blur: 0 });
    expect(document.documentElement.dataset.hasBg).toBeUndefined();
  });

  it.each([0, 0.01, 0.5, 0.99, 1])('applies opacity %s in either theme', (dim) => {
    for (const theme of ['light', 'dark'] as const) {
      applyTheme(theme);
      applyBackground({ background_url: '/theme/bg.jpg', background_dim: dim, background_blur: 0 });
      expect(document.documentElement.style.getPropertyValue('--mb-bg-dim')).toBe(String(dim));
    }
    applyBackground({ background_url: '', background_dim: 0, background_blur: 0 });
    expect(document.documentElement.style.getPropertyValue('--mb-bg-dim')).toBe('');
  });

  it('keeps custom.css as the last stylesheet', async () => {
    loadCustomCss();
    const late = document.createElement('style');
    document.head.appendChild(late);
    await new Promise((r) => setTimeout(r, 0));
    expect(document.head.lastElementChild?.id).toBe('mistbox-custom-css');
    expect((document.head.lastElementChild as HTMLLinkElement).href).toContain('/theme/custom.css');
  });
});
