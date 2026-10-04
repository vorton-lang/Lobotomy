// The WebSocket to the backend (frontend.md §3): requests with ids, pushed changes, a heartbeat,
// and reconnection. Correctness never depends on receiving every push: after reconnecting the
// client takes a new snapshot.

import type { Push, RemoteError } from './types';

export type Status = 'connecting' | 'open' | 'closed';

export class RequestFailed extends Error {
  constructor(public readonly remote: RemoteError) {
    super(remote.message);
  }
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
}

/** The server sends a tick every 15 s; a connection silent for longer is dead. */
const SILENCE_LIMIT_MS = 40_000;
const RETRY_MS = [500, 1000, 2000, 5000];

export interface ConnectionEvents {
  status(status: Status): void;
  push(push: Push): void;
  /** A new socket is open; earlier pushes may have been missed. */
  opened(): void;
}

export class Connection {
  private socket: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private lastHeard = 0;
  private watchdog: ReturnType<typeof setInterval> | null = null;
  private attempts = 0;
  private closed = false;

  constructor(
    private readonly url: string,
    private readonly events: ConnectionEvents,
  ) {}

  start() {
    this.open();
    this.watchdog = setInterval(() => {
      if (this.socket?.readyState === WebSocket.OPEN && Date.now() - this.lastHeard > SILENCE_LIMIT_MS) {
        this.socket.close();
      }
    }, 5_000);
  }

  stop() {
    this.closed = true;
    if (this.watchdog) clearInterval(this.watchdog);
    this.socket?.close();
  }

  call<T = unknown>(method: string, params: unknown = {}): Promise<T> {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new RequestFailed({ code: 'disconnected', message: '与后端的连接已断开' }));
    }
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  }

  private open() {
    this.events.status('connecting');
    const socket = new WebSocket(this.url);
    this.socket = socket;
    socket.onopen = () => {
      this.attempts = 0;
      this.lastHeard = Date.now();
      this.events.status('open');
      this.events.opened();
    };
    socket.onmessage = (event) => {
      this.lastHeard = Date.now();
      const message = JSON.parse(String(event.data));
      if ('id' in message && message.id !== null) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        if (message.error) pending.reject(new RequestFailed(message.error));
        else pending.resolve(message.result);
        return;
      }
      if ('type' in message) this.events.push(message as Push);
    };
    socket.onclose = () => {
      for (const pending of this.pending.values()) {
        pending.reject(new RequestFailed({ code: 'disconnected', message: '与后端的连接已断开' }));
      }
      this.pending.clear();
      this.events.status('closed');
      if (this.closed) return;
      const delay = RETRY_MS[Math.min(this.attempts++, RETRY_MS.length - 1)];
      setTimeout(() => this.open(), delay);
    };
  }
}
