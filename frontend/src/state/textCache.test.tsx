import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, render, renderHook, waitFor } from '@testing-library/react';
import { api, type TextPreview } from '../api';
import { TextCache, TextCacheProvider } from './textCache';
import { useText } from '../preview/text';
const sample = (text = 'hello'): TextPreview => ({ text, encoding: 'UTF-8', detected: 'UTF-8', valid_utf8: true, had_errors: false, binary: false, truncated: false, size: text.length, limit: 1024, encodings: ['UTF-8'] });
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });
it('deduplicates in-flight reads, expires entries and retries failures', async () => {
  vi.useFakeTimers();
  const cache = new TextCache();
  const fetch = vi.fn().mockResolvedValue(sample());
  const a = cache.load('key', fetch);
  expect(cache.load('key', fetch)).toBe(a);
  await a;
  expect(cache.peek('key')?.text).toBe('hello');
  expect(fetch).toHaveBeenCalledTimes(1);
  vi.advanceTimersByTime(15_001);
  expect(cache.peek('key')).toBeNull();
  await cache.load('key', fetch);
  expect(fetch).toHaveBeenCalledTimes(2);
  await expect(cache.load('bad', () => Promise.reject(new Error('offline')))).rejects.toThrow('offline');
  await cache.load('bad', fetch);
  expect(cache.peek('bad')?.text).toBe('hello');
});
it('bounds text memory and does not retain oversized previews', async () => {
  const cache = new TextCache();
  await cache.load('large', async () => sample('x'.repeat(2 * 1024 * 1024 + 1)));
  expect(cache.peek('large')).toBeNull();
  for (let i = 0; i < 17; i++) await cache.load(String(i), async () => sample());
  expect(cache.peek('0')).toBeNull();
  expect(cache.peek('16')?.text).toBe('hello');
});
it('reuses previews across tab remounts but reloads changed versions and explicit refreshes', async () => {
  const fetch = vi.spyOn(api, 'text').mockResolvedValue(sample());
  function Reader({ sha }: { sha: string }) { useText('file', sha); return null; }
  function Workspace({ shown, sha, version }: { shown: boolean; sha: string; version: number }) {
    return <TextCacheProvider version={version}>{shown ? <Reader sha={sha} /> : null}</TextCacheProvider>;
  }
  const { rerender } = render(<Workspace shown sha="v1" version={0} />);
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
  rerender(<Workspace shown={false} sha="v1" version={0} />);
  rerender(<Workspace shown sha="v1" version={0} />);
  await act(async () => {});
  expect(fetch).toHaveBeenCalledTimes(1);
  rerender(<Workspace shown sha="v2" version={0} />);
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
  rerender(<Workspace shown sha="v2" version={1} />);
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(3));
});
it('does not let a slow previous file replace the current file', async () => {
  let resolve!: (value: TextPreview) => void;
  vi.spyOn(api, 'text').mockImplementationOnce(() => new Promise((r) => { resolve = r; })).mockResolvedValue(sample('new file'));
  const wrapper = ({ children }: { children: React.ReactNode }) => <TextCacheProvider version={0}>{children}</TextCacheProvider>;
  const { result, rerender } = renderHook(({ id }) => useText(id, 'version'), { initialProps: { id: 'old' }, wrapper });
  rerender({ id: 'new' });
  await waitFor(() => expect(result.current.data?.text).toBe('new file'));
  await act(async () => resolve(sample('old file')));
  expect(result.current.data?.text).toBe('new file');
});
