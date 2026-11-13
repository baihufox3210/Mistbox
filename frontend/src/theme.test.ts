import { describe, expect, it } from 'vitest';
import { applyBackground, applyTheme, loadCustomCss, resolvedTheme } from './theme';

describe('appearance', () => {
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
    expect(document.documentElement.dataset.hasBg).toBe('true');
    applyBackground({ background_url: '', background_dim: 0, background_blur: 0 });
    expect(document.documentElement.dataset.hasBg).toBeUndefined();
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
