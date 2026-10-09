import { defineConfig } from '@playwright/test';

// End to end: a real backend of each test's own with the fake Codex (e2e/fixtures.ts), the Vite dev
// server, and the installed Edge on Windows, so no browser download is needed there; elsewhere
// Playwright's Chromium (`npx playwright install chromium`).
export default defineConfig({
  testDir: 'e2e',
  timeout: 120_000,
  // A step of the organization (writing a slot, a turn, a capture, a verification) takes seconds.
  expect: { timeout: 30_000 },
  // Fixtures own their host, project, repository and backend; independent scenarios can run
  // together, including scenarios in the same file. Keep the limit small for CI and Electron.
  fullyParallel: true,
  workers: 2,
  use: { channel: process.platform === 'win32' ? 'msedge' : undefined, trace: 'retain-on-failure' },
  webServer: { command: 'npx vite', url: 'http://127.0.0.1:5173', reuseExistingServer: true },
});
