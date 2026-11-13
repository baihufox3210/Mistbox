import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { api, type DiagnoseReport } from '../api';
import type { DriveModel } from '../state/useDrive';
import DiagnosePanel from './DiagnosePanel';
vi.mock('../state/codeFindings', () => ({ useCodeRules: () => null, SEVERITY_LABEL: {} }));
vi.mock('../preview/CodeView', () => ({ default: ({ text, lang, onChange }: { text: string; lang: string; onChange: (s: string) => void }) =>
  <textarea aria-label="測試編輯器" data-lang={lang} value={text} onChange={(e) => onChange(e.target.value)} /> }));
const model = { toast: vi.fn(), openFile: vi.fn() } as unknown as DriveModel;
const report: DiagnoseReport = { kind: 'code', lang: 'python', formatted: 'import os', first_line: 1,
  frames: [], symbols: [], fragments: [], locations: [], findings: [], solutions: [], notes: [] };
async function setup() {
  const r = render(<DiagnosePanel m={model} />);
  await screen.findByLabelText('測試編輯器');
  const clear = screen.queryByRole('button', { name: '清除' });
  if (clear) fireEvent.click(clear);
  fireEvent.change(screen.getByLabelText('程式語言'), { target: { value: 'auto' } });
  return r;
}
const change = (value: string) => fireEvent.change(screen.getByLabelText('測試編輯器'), { target: { value } });
const run = () => fireEvent.click(screen.getByRole('button', { name: '分析' }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
it('shows automatic language, stale status and manual highlighting override', async () => {
  vi.spyOn(api, 'codeDiagnose').mockResolvedValue(report);
  await setup(); change('import os'); run();
  await screen.findByText('自動判斷：Python');
  expect(screen.getByLabelText('測試編輯器').getAttribute('data-lang')).toBe('python');
  change('import sys');
  expect(screen.getByRole('status').textContent).toContain('請重新分析');
  fireEvent.change(screen.getByLabelText('程式語言'), { target: { value: 'cpp' } });
  expect(screen.getByLabelText('測試編輯器').getAttribute('data-lang')).toBe('cpp');
});
it('discards late responses after clear or edit', async () => {
  let resolve!: (r: DiagnoseReport) => void;
  vi.spyOn(api, 'codeDiagnose').mockImplementation(() => new Promise((r) => { resolve = r; }));
  await setup(); change('import os'); run();
  fireEvent.click(screen.getByRole('button', { name: '清除' }));
  await act(async () => resolve(report));
  expect(screen.queryByText('自動判斷：Python')).toBeNull();
  expect((screen.getByLabelText('測試編輯器') as HTMLTextAreaElement).value).toBe('');
  change('import os'); run(); change('changed');
  await act(async () => resolve(report));
  expect(screen.queryByText('自動判斷：Python')).toBeNull();
});
it('retains input and results across tab remounts', async () => {
  vi.spyOn(api, 'codeDiagnose').mockResolvedValue(report);
  const r = await setup(); change('import os'); run();
  await screen.findByText('自動判斷：Python'); r.unmount();
  render(<DiagnosePanel m={model} />);
  expect((await screen.findByLabelText('測試編輯器') as HTMLTextAreaElement).value).toBe('import os');
  expect(screen.getByText('自動判斷：Python')).toBeTruthy();
});
it('shows failed requests inline', async () => {
  vi.spyOn(api, 'codeDiagnose').mockRejectedValue(new Error('offline'));
  await setup(); change('import os'); run();
  expect((await screen.findByRole('alert')).textContent).toContain('offline');
});
