import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { inElectron } from './bridge';
import './styles.css';

// In the app, a window in the background dims its selection, as native windows do.
if (inElectron()) {
  const mark = () => document.documentElement.classList.toggle('window-inactive', !document.hasFocus());
  window.addEventListener('focus', mark);
  window.addEventListener('blur', mark);
  mark();
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
