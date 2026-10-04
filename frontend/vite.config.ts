import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';

// Agent output is rendered as HTML; the built app only runs its own scripts and talks only to
// the local backend. The dev server needs inline scripts for hot reload, so it goes without.
const contentSecurityPolicy: Plugin = {
  name: 'content-security-policy',
  apply: 'build',
  transformIndexHtml: () => [
    {
      tag: 'meta',
      attrs: {
        'http-equiv': 'Content-Security-Policy',
        content:
          "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; " +
          "connect-src ws://127.0.0.1:* ws://localhost:*; worker-src 'self' blob:",
      },
      injectTo: 'head-prepend',
    },
  ],
};

// The renderer. Electron loads the built files with `base: './'`; in development the browser or
// Electron loads the dev server (frontend.md §1).
export default defineConfig({
  base: './',
  plugins: [react(), contentSecurityPolicy],
  // IPv4 explicitly: `localhost` may bind only ::1 and then not answer 127.0.0.1.
  server: { host: '127.0.0.1', port: 5173, strictPort: true },
  worker: { format: 'es' },
  test: { environment: 'node', include: ['src/**/*.test.ts'] },
});
