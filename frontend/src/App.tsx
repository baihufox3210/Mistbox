import { Logo } from './components/Logo';
import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError, errorText, setCsrf, UNAUTHORIZED_EVENT } from './api';
import { Icon } from './components/Icon';
import { ShellOverlays, Topbar, useCommands } from './components/Shell';
import DriveLayout from './layouts/DriveLayout';
import IdeLayout from './layouts/IdeLayout';
import { useDrive } from './state/useDrive';
import { applyBackground, getLayout, setLayout as persistLayout, type Layout } from './theme';

type Auth = { state: 'loading' } | { state: 'out'; reason?: string } | { state: 'in'; username: string } | { state: 'error'; message: string };

export default function App() {
  const [auth, setAuth] = useState<Auth>({ state: 'loading' });

  const check = () =>
    api.me().then(
      (r) => {
        setCsrf(r.csrf_token);
        setAuth({ state: 'in', username: r.username });
      },
      (e) => setAuth(e instanceof ApiError && e.status === 401 ? { state: 'out' } : { state: 'error', message: errorText(e) }),
    );

  useEffect(() => {
    check();
    const onUnauth = () => setAuth({ state: 'out', reason: '登入已過期，請重新登入。' });
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauth);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauth);
  }, []);

  if (auth.state === 'loading') return <div className="fullscreen-state"><span className="spinner" /> 載入中…</div>;
  if (auth.state === 'error')
    return (
      <div className="fullscreen-state">
        <p>{auth.message}</p>
        <button className="btn" onClick={() => (setAuth({ state: 'loading' }), check())}>重試</button>
      </div>
    );
  if (auth.state === 'out') return <Login reason={auth.reason} onIn={(u) => setAuth({ state: 'in', username: u })} />;
  return (
    <Workspace
      username={auth.username}
      onLogout={async () => {
        try {
          await api.logout();
        } finally {
          setCsrf('');
          setAuth({ state: 'out' });
        }
      }}
    />
  );
}

function Workspace({ username, onLogout }: { username: string; onLogout: () => void }) {
  const m = useDrive();
  const [layout, setLayoutState] = useState<Layout>(getLayout());
  const [sideOpen, setSideOpen] = useState(false);
  const targetRef = useRef<() => string>(() => 'root');
  const mRef = useRef(m);
  mRef.current = m;
  const setLayout = useCallback((l: Layout) => {
    persistLayout(l);
    // Clear the preview before changing layouts; keep file tabs and drafts.
    if (l === 'drive') mRef.current.setActive(null);
    setLayoutState(l);
  }, []);
  const target = useCallback(() => targetRef.current(), []);
  const commands = useCommands(m, { target, layout, setLayout, onLogout });

  useEffect(() => {
    api.settings().then(applyBackground, () => {});
  }, []);

  return (
    <div className={`app layout-${layout}`} data-region="app">
      <Topbar
        m={m}
        username={username}
        layout={layout}
        setLayout={setLayout}
        onLogout={onLogout}
        leading={
          <button className="btn ghost icon-only mobile-only" onClick={() => setSideOpen((o) => !o)} aria-label="側欄">
            <Icon name="menu" />
          </button>
        }
      />
      {layout === 'ide' ? (
        <IdeLayout m={m} sideOpen={sideOpen} setSideOpen={setSideOpen} targetRef={targetRef} />
      ) : (
        <DriveLayout m={m} sideOpen={sideOpen} setSideOpen={setSideOpen} targetRef={targetRef} setLayout={setLayout} />
      )}
      <ShellOverlays m={m} commands={commands} layout={layout} setLayout={setLayout} />
    </div>
  );
}

function Login({ onIn, reason }: { onIn: (u: string) => void; reason?: string }) {
  const [username, setU] = useState('');
  const [password, setP] = useState('');
  const [err, setErr] = useState(reason ?? '');
  const [busy, setBusy] = useState(false);
  return (
    <main className="login">
      <form
        className="login-card"
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          setErr('');
          try {
            const r = await api.login(username.trim(), password);
            setCsrf(r.csrf_token);
            setP('');
            onIn(r.username);
          } catch (e) {
            setErr(errorText(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <div className="brand big">
          <Logo size={40} />
          <h1>Mistbox</h1>
        </div>
        <p className="muted">私人雲端硬碟</p>
        <label className="field">
          帳號
          <input autoComplete="username" value={username} onChange={(e) => setU(e.target.value)} required autoFocus />
        </label>
        <label className="field">
          密碼
          <input type="password" autoComplete="current-password" value={password} onChange={(e) => setP(e.target.value)} required />
        </label>
        {err && <p className="form-error" role="alert">{err}</p>}
        <button className="btn primary block" disabled={busy}>
          {busy ? '登入中…' : '登入'}
        </button>
        <p className="muted small">尚未建立帳號？請由管理者在伺服器上執行 <code>mistbox admin create</code>。</p>
      </form>
    </main>
  );
}
