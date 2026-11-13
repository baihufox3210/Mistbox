import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import SharePage, { shareIdFromPath } from './SharePage';
import './styles.css';
import { initTheme, loadCustomCss } from './theme';

initTheme();
loadCustomCss();

// /s/<id>#<secret> is a share link; everything else is the admin app.
const shareId = shareIdFromPath();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {shareId ? <SharePage id={shareId} /> : <App />}
  </StrictMode>,
);
