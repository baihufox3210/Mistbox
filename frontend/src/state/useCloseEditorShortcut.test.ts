import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, renderHook } from '@testing-library/react';
import { useCloseEditorShortcut } from './useCloseEditorShortcut';
afterEach(cleanup);
function press(init: KeyboardEventInit) {
  const e = new KeyboardEvent('keydown', { key: 'w', bubbles: true, cancelable: true, ...init });
  window.dispatchEvent(e);
  return e;
}
it('closes the active file for Ctrl/Cmd+W and Alt+W and cancels the delivered browser event', () => {
  const closeTab = vi.fn();
  renderHook(() => useCloseEditorShortcut({ active: 'file', dialog: null, closeTab }));
  for (const mod of [{ ctrlKey: true }, { metaKey: true }, { altKey: true }]) expect(press(mod).defaultPrevented).toBe(true);
  expect(closeTab.mock.calls).toEqual([['file'], ['file'], ['file']]);
});
it('ignores repeats, dialogs, IME and modified browser window shortcuts; removes listener outside IDE', () => {
  const closeTab = vi.fn();
  const hook = renderHook(({ dialog }) => useCloseEditorShortcut({ active: 'file', dialog, closeTab }), { initialProps: { dialog: null as null | { t: 'confirm-close'; id: string } } });
  press({ ctrlKey: true, repeat: true });
  press({ ctrlKey: true, isComposing: true });
  expect(press({ ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(false);
  hook.rerender({ dialog: { t: 'confirm-close', id: 'file' } });
  expect(press({ ctrlKey: true }).defaultPrevented).toBe(true);
  expect(closeTab).not.toHaveBeenCalled();
  hook.unmount();
  expect(press({ ctrlKey: true }).defaultPrevented).toBe(false);
});
