// Where the Vite dev server listens: Vite, Playwright, the end-to-end tests and Electron's --dev
// read it here. LOBOTOMY_PORT_BASE gives a working directory its own port, so several working
// directories can run their dev servers and end-to-end tests at the same time.
export const devPort = Number(process.env.LOBOTOMY_PORT_BASE || 5173);
export const devOrigin = `http://127.0.0.1:${devPort}`;
