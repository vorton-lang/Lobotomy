import { defineConfig } from '@playwright/test';

// The performance baseline (frontend.md §5): the production build served by `vite preview`, the
// release backend with the fake Codex (bench/setup.ts). Edge on Windows, Playwright's Chromium
// elsewhere; both are Chromium, whose DevTools protocol gives the memory figures.
export default defineConfig({
  testDir: '.',
  testMatch: 'baseline.spec.ts',
  timeout: 300_000,
  expect: { timeout: 30_000 },
  workers: 1,
  globalSetup: './setup.ts',
  reporter: 'list',
  use: {
    baseURL: 'http://127.0.0.1:4173',
    channel: process.env.BENCH_CHANNEL ?? (process.platform === 'win32' ? 'msedge' : undefined),
  },
  webServer: {
    command: 'npx vite preview --host 127.0.0.1 --port 4173 --strictPort',
    cwd: '..',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: false,
  },
});
