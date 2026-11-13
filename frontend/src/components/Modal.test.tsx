import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { Modal } from './Modal';
afterEach(cleanup);
it('keeps input focus across rerenders and calls the latest close callback', () => {
  const oldClose = vi.fn();
  const newClose = vi.fn();
  const content = <><input aria-label="第一欄" /><input aria-label="第二欄" /></>;
  const r = render(<Modal title="編輯" onClose={oldClose}>{content}</Modal>);
  const second = screen.getByLabelText('第二欄');
  second.focus();
  r.rerender(<Modal title="編輯" onClose={newClose}>{content}</Modal>);
  expect(document.activeElement).toBe(second);
  fireEvent.keyDown(window, { key: 'Escape' });
  expect(newClose).toHaveBeenCalledOnce();
  expect(oldClose).not.toHaveBeenCalled();
});
it('Escape closes only the topmost dialog', () => {
  const parent = vi.fn();
  const child = vi.fn();
  render(<Modal title="外層" onClose={parent}><Modal title="內層" onClose={child}><button>確認</button></Modal></Modal>);
  fireEvent.keyDown(window, { key: 'Escape' });
  expect(child).toHaveBeenCalledOnce();
  expect(parent).not.toHaveBeenCalled();
});
