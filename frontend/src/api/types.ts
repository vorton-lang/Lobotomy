// What the backend's GUI service sends (backend/crates/lobotomyd/src/gui.rs). Field names follow
// the Rust structs.

export type Phase = 'queued' | 'executing' | 'verifying' | 'accepting' | 'done' | 'abandoned';

export interface Project {
  id: string;
  repo_path: string;
  branch: string;
  author_name: string;
  author_email: string;
  integration: string;
  integration_rev: number;
  previewed: string;
}

export interface Check {
  command: string;
  timeout_secs: number;
}

export interface ProjectConfig {
  checks: Check[];
  excluded: string[];
  force_tracked: string[];
  max_new_files: number;
  max_new_bytes: number;
}

export interface Failure {
  kind: 'quota' | 'other';
  message: string;
  resets_at: number | null;
}

export interface Turn {
  id: string;
  role: string;
  task_id: string | null;
  attempt_id: string | null;
  input: string;
  state: 'registered' | 'running' | 'ended' | 'unknown';
  outcome: 'completed' | 'failed' | 'interrupted' | null;
  failure: Failure | null;
  pid: number | null;
  done_at: number | null;
  registered_at: number;
  started_at: number | null;
  ended_at: number | null;
}

export interface Verification {
  id: string;
  task_id: string;
  capture_id: string;
  base: string;
  config_version: number;
  commit_id: string | null;
  conflicts: string[];
  state: 'running' | 'passed' | 'failed';
}

export interface Task {
  id: string;
  title: string;
  body: string;
  executor: string;
  phase: Phase;
  paused: boolean;
  blocked_reason: string | null;
  criteria_version: number;
  queue_pos: number | null;
  revision: number;
  created_at: number;
  closed_at: number | null;
  attempt_seq: number | null;
  attempt_open: boolean;
  verification: Verification | null;
  /** The user's messages its executor never got; accepting needs the user to let them go. */
  undelivered_messages: number;
  /** For a task made from changes outside any task: their capture. */
  origin_capture: string | null;
}

export interface NewFile {
  path: string;
  size: number;
}

export interface UncoveredPath {
  kind: string;
  path: string;
}

export interface Capture {
  id: string;
  turn_id: string;
  role: string;
  task_id: string | null;
  attempt_id: string | null;
  kind: 'turn' | 'candidate' | 'interrupted';
  base: string;
  state: 'intent' | 'pinned' | 'oversized' | 'uncovered';
  commit_id: string | null;
  detail: { files?: NewFile[]; total_bytes?: number; paths?: UncoveredPath[]; changed?: string[] } | null;
  created_at: number;
  /** Set for a capture outside any task that changed something. */
  outside: 'pending' | 'adopted' | 'discarded' | null;
}

export type Hold =
  | { kind: 'abnormal'; turn: Turn }
  | { kind: 'capture_stopped'; capture: Capture }
  | { kind: 'session_unidentified'; session_id: string };

export interface Workspace {
  id: string;
  name: string;
  generation: number;
  target: string;
  head: string;
  task_id: string | null;
  state: 'materializing' | 'ready' | 'retired';
}

export interface RoleView {
  name: string;
  kind: string;
  harness: string;
  model: string | null;
  slot: string | null;
  task_id: string | null;
  unfinished: Turn | null;
  last_turn: Turn | null;
  hold: Hold | null;
  stalled: Stalled | null;
  /** Changes a turn outside any task left in the slot, waiting for the user. */
  outside: Capture | null;
  workspace: Workspace | null;
  queued_messages: number;
}

/** The executing task waits on the user: the last turn ended without done or a question. */
export interface Stalled {
  task_id: string;
  turn_id: string;
  report_error: string | null;
}

export interface Domain {
  harness: string;
  account: string;
  blocked_at: number | null;
  resets_at: number | null;
  message: string | null;
}

export type Attention =
  | { kind: 'hold'; role: string; hold: Hold }
  | { kind: 'outside_changes'; role: string; capture: Capture }
  | { kind: 'unknown_turn'; role: string; turn_id: string }
  | { kind: 'task_blocked'; task_id: string; title: string; reason: string }
  | ({ kind: 'stalled'; role: string; title: string } & Stalled)
  | { kind: 'accept'; task_id: string; title: string; verification_id: string }
  | { kind: 'quota'; domain: Domain }
  | { kind: 'job_failed'; key: string; reason: string }
  | { kind: 'preview_stopped'; reason: string };

export interface LiveItem {
  kind: string;
  content: Record<string, unknown>;
  started_at: number;
}

export interface LiveTurn {
  role: string;
  started_at: number;
  items: Record<string, LiveItem>;
}

export interface Snapshot {
  seq: number;
  project: Project | null;
  config: { version: number; config: ProjectConfig } | null;
  roles: RoleView[];
  tasks: Task[];
  attention: Attention[];
  quota: Domain[];
  live: Record<string, LiveTurn>;
}

/** A field moved to the blob store because it was large (data-model.md §7.3). */
export interface BlobRef {
  blob: string;
  size: number;
  head: string;
  tail: string;
}

export interface Item {
  id: string;
  seq: number;
  turn_id: string;
  kind: string;
  content: Record<string, unknown>;
  command_id: string | null;
  created_at: number;
}

export interface Message {
  id: string;
  source: string;
  task_id: string | null;
  body: string;
  turn_id: string | null;
  created_at: number;
}

export interface CommandRow {
  name: string;
  args: Record<string, unknown>;
  result: unknown;
}

export interface ThreadPage {
  items: Item[];
  has_more: boolean;
  turns: Record<string, Turn>;
  messages: Message[];
  commands: Record<string, CommandRow>;
  queued: Message[];
}

/** A search match: an item, or a message shown at its turn's first item (`seq` null: queued). */
export interface SearchMatch {
  kind: 'item' | 'message';
  id: string;
  seq: number | null;
}

export type Content =
  | { kind: 'text'; text: string }
  | { kind: 'binary'; size: number }
  | { kind: 'too_large'; size: number }
  | { kind: 'symlink'; target: string }
  | { kind: 'conflict' };

export interface FileChange {
  path: string;
  old: Content | null;
  new: Content | null;
}

export interface CheckRunRow {
  id: string;
  verification_id: string;
  seq: number;
  command: string;
  exit_code: number | null;
  timed_out: number;
  output: { stdout?: string | BlobRef; stderr?: string | BlobRef };
  duration_ms: number;
}

export interface Attempt {
  id: string;
  seq: number;
  started_at: number;
  ended_at: number | null;
  end_reason: string | null;
  candidate_id: string | null;
}

export type VerificationRow = Omit<Verification, 'task_id'> & { created_at: number; finished_at: number | null; attempt_id: string };

export interface TaskDetail {
  task: Omit<Task, 'attempt_seq' | 'attempt_open' | 'verification' | 'undelivered_messages'>;
  criteria: { version: number; text: string; created_by: string; created_at: number }[];
  attempts: Attempt[];
  captures: Capture[];
  verifications: VerificationRow[];
  checks: CheckRunRow[];
  decisions: { id: string; kind: string; actor: string; detail: Record<string, unknown>; created_at: number }[];
  publications: { rev: number; commit_id: string; previous: string; created_at: number }[];
  /** The user's messages to the task that its executor never got (data-model.md §4.2). */
  undelivered_messages: { id: string; body: string }[];
}

export interface RemoteError {
  code: string;
  message: string;
}

export type Push =
  | { type: 'events'; events: { seq: number; kind: string; entity: string; payload: Record<string, unknown> }[] }
  | { type: 'live'; live: Record<string, LiveTurn> }
  | { type: 'thread'; role: string; seq: number }
  | { type: 'tick' }
  | { type: 'resync' };
