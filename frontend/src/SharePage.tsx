import { Logo } from './components/Logo';
// Page for people holding a share link: /s/<id>#<secret>.
// No login. The secret is read from the URL fragment (never sent to the
// server as part of the URL) and sent in the X-Share-Token header.
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, ApiError, errorText, setShare, type OpenedShare } from './api';
import { CapsContext, type Caps } from './caps';
import { Icon } from './components/Icon';
import { MOD, ShellOverlays, useCommands } from './components/Shell';
import { formatDate } from './format';
import DriveLayout from './layouts/DriveLayout';
import { useDrive } from './state/useDrive';
import { applyTheme, resolvedTheme } from './theme';

const Preview = lazy(() => import('./preview/Preview'));

type State = { s: 'loading' } | { s: 'error'; message: string; invalid?: boolean } | { s: 'ok'; share: OpenedShare };

export function shareIdFromPath(path = window.location.pathname): string | null {
  const m = /^\/s\/([0-9a-f]{16})\/?$/.exec(path);
  return m ? m[1] : null;
}

export default function SharePage({ id }: { id: string }) {
  const [st, setSt] = useState<State>({ s: 'loading' });
  const open = useCallback(() => {
    const secret = window.location.hash.replace(/^#/, '');
    if (!/^[0-9a-f]{64}$/.test(secret)) {
      setSt({ s: 'error', message: '連結不完整：請確認複製了完整的網址（包含 # 之後的部分）。', invalid: true });
      return;
    }
    setShare(id, secret);
    setSt({ s: 'loading' });
    api.shareOpen().then(
      (share) => {
        document.title = `${share.root.name} · Mistbox 分享`;
        setSt({ s: 'ok', share });
      },
      (e) =>
        setSt(
          e instanceof ApiError && e.code === 'share_invalid'
            ? { s: 'error', message: '這個分享連結無效、已過期，或已被分享者取消。', invalid: true }
            : { s: 'error', message: errorText(e) },
        ),
    );
  }, [id]);
  useEffect(open, [open]);

  if (st.s === 'loading') return <div className="fullscreen-state"><span className="spinner" /> 開啟分享…</div>;
  if (st.s === 'error')
    return (
      <div className="fullscreen-state share-error">
        <Logo size={48} />
        <h2>{st.invalid ? '無法開啟分享' : '發生錯誤'}</h2>
        <p className="muted">{st.message}</p>
        {!st.invalid && <button className="btn" onClick={open}>重試</button>}
      </div>
    );
  const caps: Caps = { admin: false, write: st.share.permission === 'edit', rootName: st.share.root.name };
  return (
    <CapsContext.Provider value={caps}>
      {st.share.root.kind === 'dir' ? <SharedFolder share={st.share} /> : <SharedFile share={st.share} />}
    </CapsContext.Provider>
  );
}

function ShareBar({ share, onPalette, leading }: { share: OpenedShare; onPalette?: () => void; leading?: React.ReactNode }) {
  const [dark, setDark] = useState(resolvedTheme() === 'dark');
  useEffect(() => {
    const f = () => setDark(resolvedTheme() === 'dark');
    window.addEventListener('mistbox:theme', f);
    return () => window.removeEventListener('mistbox:theme', f);
  }, []);
  const edit = share.permission === 'edit';
  return (
    <header className="topbar share-bar" data-region="topbar">
      {leading}
      <div className="brand">
        <Logo size={24} />
        <span>Mistbox</span>
      </div>
      <div className="share-title">
        <strong title={share.root.name}>{share.root.name}</strong>
        <span className={`badge ${edit ? 'warn' : 'ok'}`}>
          <Icon name={edit ? 'edit' : 'eye'} size={12} /> {edit ? '可編輯' : '僅檢視'}
        </span>
        <span className="muted small hide-sm">{share.expires_at ? `有效至 ${formatDate(share.expires_at)}` : '分享連結'}</span>
      </div>
      {onPalette && (
        <button className="cmd-center" onClick={onPalette} title="搜尋與指令（Ctrl+Shift+P / F1 / Ctrl+K）">
          <Icon name="search" />
          <span className="cmd-text">搜尋檔案、指令…</span>
          <kbd>{MOD}+Shift+P</kbd>
        </button>
      )}
      <div className="topbar-actions">
        <button className="btn ghost icon-only" onClick={() => applyTheme(dark ? 'light' : 'dark')} title={dark ? '切換為淺色' : '切換為深色'} aria-label="切換主題">
          <Icon name={dark ? 'sun' : 'moon'} />
        </button>
      </div>
    </header>
  );
}

function SharedFolder({ share }: { share: OpenedShare }) {
  const m = useDrive({ admin: false });
  const [sideOpen, setSideOpen] = useState(false);
  const targetRef = useRef<() => string>(() => 'root');
  const target = useCallback(() => targetRef.current(), []);
  const noop = useCallback(() => {}, []);
  const commands = useCommands(m, { target, layout: 'drive', setLayout: noop, onLogout: noop });
  return (
    <div className="app layout-drive share-app" data-region="app">
      <ShareBar
        share={share}
        onPalette={() => m.setPaletteMode('all')}
        leading={
          <button className="btn ghost icon-only mobile-only" onClick={() => setSideOpen((o) => !o)} aria-label="側欄">
            <Icon name="menu" />
          </button>
        }
      />
      <DriveLayout m={m} sideOpen={sideOpen} setSideOpen={setSideOpen} targetRef={targetRef} setLayout={noop} />
      <ShellOverlays m={m} commands={commands} layout="drive" setLayout={noop} />
    </div>
  );
}

function SharedFile({ share }: { share: OpenedShare }) {
  const [node, setNode] = useState(share.root);
  const hooks = useMemo(() => ({ onSaved: setNode }), []);
  return (
    <div className="app share-app" data-region="app">
      <ShareBar share={share} />
      <main className="share-file">
        <Suspense fallback={<div className="state"><span className="spinner" /> 載入預覽…</div>}>
          <Preview node={node} onOpenPath={() => {}} editor={hooks} />
        </Suspense>
      </main>
    </div>
  );
}
