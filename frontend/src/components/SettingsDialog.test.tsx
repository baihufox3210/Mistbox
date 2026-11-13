import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api, type UiSettings } from '../api';
import { applyBackground } from '../theme';
import SettingsDialog from './SettingsDialog';
vi.mock('../theme', () => ({
  applyBackground: vi.fn(), applyLogo: vi.fn(), applyTheme: vi.fn(), loadCustomCss: vi.fn(),
  getThemePref: () => 'system', getLogoPref: () => 'auto',
}));
const saved: UiSettings = { background_url: 'https://example.com/background.jpg', background_dim: 0.3, background_blur: 4, theme_css_url: null };
beforeEach(() => {
  vi.spyOn(api, 'settings').mockResolvedValue(saved);
  vi.spyOn(api, 'ragStatus').mockResolvedValue({ enabled: true, state: 'ready', indexed_files: 3 });
  vi.spyOn(api, 'codeStatus').mockResolvedValue({ state: 'ready', files: 2, pending: 0, by_lang: { c: 2 }, findings: {}, notes: 1, semantic: null });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.clearAllMocks(); });
async function setup() {
  const close = vi.fn();
  render(<SettingsDialog onClose={close} layout="ide" setLayout={vi.fn()} />);
  await waitFor(() => expect(api.settings).toHaveBeenCalledOnce());
  return close;
}
it('loads service status only when requested and surfaces failures', async () => {
  vi.mocked(api.codeStatus).mockRejectedValue(new Error('連線中斷'));
  await setup();
  expect(api.ragStatus).not.toHaveBeenCalled();
  expect(screen.queryByLabelText('圖片網址')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: '系統' }));
  expect((await screen.findByRole('alert')).textContent).toContain('連線中斷');
  expect(api.ragStatus).toHaveBeenCalledOnce();
});
it('preserves background edits between settings pages and restores saved preview on close', async () => {
  const close = await setup();
  fireEvent.click(screen.getByRole('button', { name: '背景' }));
  const url = await screen.findByLabelText('圖片網址');
  fireEvent.change(url, { target: { value: 'https://example.com/new.jpg' } });
  const blur = screen.getByRole('slider', { name: /^模糊/ });
  blur.focus();
  fireEvent.change(blur, { target: { value: '8' } });
  expect(document.activeElement).toBe(blur);
  fireEvent.click(screen.getByRole('button', { name: '外觀' }));
  fireEvent.click(screen.getByRole('button', { name: '背景' }));
  expect((screen.getByLabelText('圖片網址') as HTMLInputElement).value).toBe('https://example.com/new.jpg');
  fireEvent.click(screen.getByRole('button', { name: '關閉' }));
  expect(applyBackground).toHaveBeenLastCalledWith(saved);
  expect(close).toHaveBeenCalledOnce();
});
it('saves the background and keeps errors actionable', async () => {
  const save = vi.spyOn(api, 'saveSettings').mockResolvedValue({ ...saved, background_url: '' });
  await setup();
  fireEvent.click(screen.getByRole('button', { name: '背景' }));
  await screen.findByLabelText('圖片網址');
  fireEvent.click(screen.getByRole('button', { name: '移除背景' }));
  fireEvent.click(screen.getByRole('button', { name: '儲存背景' }));
  await screen.findByText('背景已儲存');
  expect(save).toHaveBeenCalledWith({ background_url: '', background_dim: 0.3, background_blur: 4 });
  save.mockRejectedValueOnce(new Error('儲存失敗'));
  fireEvent.change(screen.getByLabelText('圖片網址'), { target: { value: 'https://example.com/next.jpg' } });
  fireEvent.click(screen.getByRole('button', { name: '儲存背景' }));
  expect((await screen.findByRole('alert')).textContent).toBe('儲存失敗');
});

it('opens service status directly from a contextual settings entry', async () => {
  render(<SettingsDialog onClose={vi.fn()} layout="ide" setLayout={vi.fn()} section="system" />);
  await screen.findByText('2 個檔案 · 1 篇筆記');
  expect(screen.getByRole('button', { name: '系統' }).getAttribute('aria-current')).toBe('page');
  expect(screen.queryByRole('group', { name: '主題' })).toBeNull();
});
