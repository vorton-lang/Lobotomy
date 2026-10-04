// What Electron's preload offers (electron/preload.cjs). In a plain browser, during development
// and in end-to-end tests, the backend's address and token come from the URL:
// `?backend=ws://127.0.0.1:PORT/gui&token=…`.

export interface LobotomyBridge {
  /** The running backend of the current project, or `null` when there is no project yet. */
  connection(): Promise<{ url: string; token: string } | null>;
  /** Lets the user pick a repository, then starts a backend for a new project. */
  chooseRepo(): Promise<string | null>;
  setAttention(count: number): void;
  openExternal(url: string): void;
}

declare global {
  interface Window {
    lobotomy?: LobotomyBridge;
  }
}

export const inElectron = () => window.lobotomy !== undefined;

export async function backendUrl(): Promise<string | null> {
  const params = new URLSearchParams(location.search);
  const backend = params.get('backend');
  const token = params.get('token');
  if (backend && token) return `${backend}?token=${encodeURIComponent(token)}`;
  const info = await window.lobotomy?.connection();
  return info ? `${info.url}?token=${encodeURIComponent(info.token)}` : null;
}
