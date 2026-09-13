import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './app/App';
import { Providers } from './app/providers';
import './styles/tokens.css';
import './styles/base.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Providers>
      <App />
    </Providers>
  </React.StrictMode>,
);

// Fade the launch splash once React has painted the shell. The CSS also
// follows the OS appearance so dark mode never flashes white.
const splash = document.getElementById('sift-splash');
if (splash) {
  requestAnimationFrame(() => {
    splash.classList.add('hide');
    window.setTimeout(() => splash.remove(), 240);
  });
}
