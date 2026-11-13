import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { api } from '../api';
import { DIAGNOSE_TAB_ID, useDrive } from './useDrive';
vi.mock('../components/Uploads', () => ({ useUploads: () => ({}), filesFromDrop: vi.fn() }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); window.history.replaceState(null, '', '/'); });
it('opens one temporary tab, excludes it from file URLs and closes without storage calls', async () => {
  vi.spyOn(api, 'children').mockResolvedValue({ node: { id: 'root' }, children: [], breadcrumbs: [] } as never);
  const info = vi.spyOn(api, 'info');
  const { result } = renderHook(() => useDrive());
  await act(async () => {});
  act(() => { result.current.openDiagnose(); result.current.openDiagnose(); });
  expect(result.current.tabs).toHaveLength(1);
  expect(result.current.tabs[0].kind).toBe('diagnose');
  expect(result.current.active).toBe(DIAGNOSE_TAB_ID);
  act(() => result.current.setActive(DIAGNOSE_TAB_ID));
  expect(window.location.search).not.toContain('file=');
  act(() => result.current.closeTab(DIAGNOSE_TAB_ID));
  expect(result.current.tabs).toHaveLength(0);
  expect(info).not.toHaveBeenCalled();
});

it('retains a dirty file and opens confirmation when closing through the shared action', async () => {
  vi.spyOn(api, 'children').mockResolvedValue({ node: { id: 'root' }, children: [], breadcrumbs: [] } as never);
  const { result } = renderHook(() => useDrive());
  await act(async () => {});
  act(() => result.current.openFile({ id: 'file', parent_id: 'root', name: 'main.c', kind: 'file', size: 0, mime: null, sha256: 'sha', sensitive: false, created_at: '', modified_at: '' }));
  act(() => result.current.setDirty('file', true));
  act(() => result.current.closeTab('file'));
  expect(result.current.tabs).toHaveLength(1);
  expect(result.current.dialog).toEqual({ t: 'confirm-close', id: 'file' });
});
