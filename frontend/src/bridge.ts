// What Electron's preload offers (electron/preload.cjs). In a plain browser, during development
// and in end-to-end tests, the backend's address and token come from the URL:
// `?backend=ws://127.0.0.1:PORT/gui&token=…`.

export interface LobotomyBridge {
  /** The running backend of the current project, or `null` when there is no project yet. */
  connection(): Promise<{ url: string; token: string } | null>;
  /** The project's backend if one is running now; never starts one. */
  find(): Promise<{ url: string; token: string } | null>;
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

const withToken = (info: { url: string; token: string } | null | undefined) =>
  info ? `${info.url}?token=${encodeURIComponent(info.token)}` : null;

export async function backendUrl(): Promise<string | null> {
  const params = new URLSearchParams(location.search);
  const backend = params.get('backend');
  const token = params.get('token');
  if (backend && token) return `${backend}?token=${encodeURIComponent(token)}`;
  return withToken(await window.lobotomy?.connection());
}

/**
 * Where the backend listens now, for reconnecting after it restarted on another port. Only the app
 * can tell; an address given in the URL stays as it is.
 */
export async function findBackend(): Promise<string | null> {
  return withToken(await window.lobotomy?.find());
}
