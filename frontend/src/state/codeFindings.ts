// Static-analysis results from the code index (/api/v1/code/*).
import { useEffect, useState } from 'react';
import { api, type CodeFinding, type CodeRule, type NodeInfo } from '../api';

let rulesP: Promise<Map<string, CodeRule>> | null = null;

/** Rule catalogue (fetched once). */
export function useCodeRules(): Map<string, CodeRule> | null {
  const [rules, setRules] = useState<Map<string, CodeRule> | null>(null);
  useEffect(() => {
    rulesP ??= api.codeRules().then(
      (r) => new Map(r.rules.map((x) => [x.id, x])),
      (e) => {
        rulesP = null;
        throw e;
      },
    );
    let live = true;
    rulesP.then((m) => live && setRules(m), () => {});
    return () => {
      live = false;
    };
  }, []);
  return rules;
}

export type FindingsState =
  | { s: 'off' }
  | { s: 'loading' }
  /** The file changed and is being re-analysed; `findings` are from the last run. */
  | { s: 'pending'; findings: CodeFinding[] }
  | { s: 'ready'; findings: CodeFinding[] }
  | { s: 'skipped'; detail: string }
  | { s: 'error' };

/**
 * Findings for one file. While the index has not caught up with the file's
 * current content (just saved / uploaded), polls every 2 s for a while.
 */
export function useFileFindings(node: NodeInfo | undefined, enabled: boolean): FindingsState {
  const [st, setSt] = useState<FindingsState>({ s: 'off' });
  const id = node?.id;
  const sha = node?.sha256 ?? '';
  useEffect(() => {
    if (!enabled || !id) return setSt({ s: 'off' });
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let tries = 0;
    setSt((p) => (p.s === 'ready' || p.s === 'pending' ? { s: 'pending', findings: p.findings } : { s: 'loading' }));
    const load = () =>
      api.codeFindings(id).then(
        (r) => {
          if (!live) return;
          const current = !!r.index && r.index.sha256 === sha;
          if (current && r.index!.status !== 'indexed') return setSt({ s: 'skipped', detail: r.index!.detail ?? '' });
          if (current || tries >= 15) return setSt({ s: 'ready', findings: r.findings });
          setSt({ s: 'pending', findings: r.findings });
          tries += 1;
          timer = setTimeout(load, 2000);
        },
        () => live && setSt({ s: 'error' }),
      );
    load();
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [id, sha, enabled]);
  return st;
}

export const SEVERITY_LABEL = { error: '錯誤', warning: '警告', perf: '可優化', info: '建議' } as const;
