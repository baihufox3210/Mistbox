// "Open / new note" for a rule or a known error: notes link themselves with
// front matter (`rules: [omp-race]`, `known: [undefined-reference]`). If such
// a note exists it is opened; otherwise a template is created under /筆記/.
import { useEffect, useState } from 'react';
import { api, ApiError, errorText, type NodeInfo } from '../api';
import type { DriveModel } from '../state/useDrive';
import { Icon } from './Icon';

export interface NoteTarget {
  kind: 'rule' | 'known';
  id: string;
  title: string;
  /** Explanation / fix steps written into a new note. */
  steps: string[];
}

export const NOTES_ROOT = '/筆記';

export function noteTemplate(t: NoteTarget): string {
  const key = t.kind === 'rule' ? 'rules' : 'known';
  const intro =
    t.kind === 'rule'
      ? `> 規則 \`${t.id}\`：${t.steps.join(' ')}`
      : t.steps.map((s) => `- ${s}`).join('\n');
  return `---
${key}: [${t.id}]
# 也可以加上：symbols: [函式名稱]、errors: ["錯誤訊息中的一段文字"]、tags: [rp2350]
---
# ${t.title}

${intro}

## 我遇到的情況

## 解法與心得
`;
}

export function notePath(t: NoteTarget): string {
  return `${NOTES_ROOT}/${t.kind === 'rule' ? '規則' : '錯誤'}/${t.id}.md`;
}

async function openOrCreate(m: DriveModel, t: NoteTarget, existing: NodeInfo[]): Promise<boolean> {
  try {
    if (existing.length) {
      m.openFile(await api.info(existing[0].id));
      return true;
    }
    let n: NodeInfo;
    try {
      n = await api.createTextAt(notePath(t), noteTemplate(t));
      m.toast(`已建立筆記 ${n.path ?? notePath(t)}`);
      m.bump();
    } catch (e) {
      // Same file name already there (without the tag): open it.
      if (!(e instanceof ApiError && e.status === 409)) throw e;
      n = await api.infoPath(notePath(t));
    }
    m.openFile(n);
    return true;
  } catch (e) {
    m.toast(errorText(e));
    return false;
  }
}

export default function RuleNoteButton({ m, target, onOpened }: { m: DriveModel; target: NoteTarget; onOpened?: () => void }) {
  const [notes, setNotes] = useState<NodeInfo[] | null>(null);
  useEffect(() => {
    let live = true;
    api.codeNotes(target.kind === 'rule' ? { rule: target.id } : { known: target.id }).then(
      (r) => live && setNotes(r.notes),
      () => live && setNotes([]),
    );
    return () => {
      live = false;
    };
  }, [target.kind, target.id]);
  const has = (notes?.length ?? 0) > 0;
  return (
    <button
      className="btn ghost xs note-btn"
      disabled={!notes}
      onClick={async (e) => {
        e.stopPropagation();
        if (notes && (await openOrCreate(m, target, notes))) onOpened?.();
      }}
      title={has ? notes!.map((n) => n.path).join('\n') : `在 ${notePath(target)} 建立筆記（含 ${target.kind === 'rule' ? 'rules' : 'known'}: [${target.id}] 標註）`}
    >
      <Icon name={has ? 'file' : 'filePlus'} size={13} /> {has ? `開啟筆記${notes!.length > 1 ? `（${notes!.length}）` : ''}` : '新增筆記'}
    </button>
  );
}
