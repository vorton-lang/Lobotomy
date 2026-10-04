import { defineConfig } from '@playwright/test';

// End to end: a real backend with the fake Codex (e2e/setup.ts), the Vite dev server, and the
// installed Edge, so no browser download is needed.
export default defineConfig({
  testDir: 'e2e',
  timeout: 120_000,
  // A step of the organization (writing a slot, a turn, a capture, a verification) takes seconds.
  expect: { timeout: 30_000 },
  workers: 1,
  globalSetup: './e2e/setup.ts',
  use: { channel: 'msedge', trace: 'retain-on-failure' },
  webServer: { command: 'npx vite', url: 'http://127.0.0.1:5173', reuseExistingServer: true },
});
