// What the backend's GUI service sends: the read models of `lobotomy_core::view`
// (backend/crates/core/src/view/) and what `backend/crates/lobotomyd/src/gui/` adds. Field names
// follow the Rust structs.

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
  /** `permission`: the environment does not allow the harness's permission mode. */
  kind: 'quota' | 'permission' | 'other';
  message: string;
  resets_at: number | null;
  /** The CLI wrote nothing on stdout: the turn never began, and continuing sends its input again. */
  unstarted?: boolean;
}

/** What a role's CLI may do without asking; there is no mode that asks the user. */
export type Permission = 'full' | 'auto_review';

/** A harness's host settings, shared by all projects. */
export interface HarnessView {
  harness: string;
  permission: Permission;
}

export interface Turn {
  id: string;
  role: string;
  harness: string;
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
  attempt_id: string;
  capture_id: string;
  /** The integration version the candidate was rebased onto. */
  base: string;
  config_version: number;
  /** The rebased commit: the next integration version if the user accepts it. */
  commit_id: string | null;
  conflicts: string[];
  state: 'running' | 'passed' | 'failed';
  created_at: number;
  finished_at: number | null;
}

/** A task as stored. */
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
  /** For a task made from changes outside any task: their capture. */
  origin_capture: string | null;
}

/** A task in the snapshot: the record and where its work stands. */
export interface TaskView extends Task {
  attempt_seq: number | null;
  attempt_open: boolean;
  verification: Verification | null;
  /** The user's messages its executor never got; accepting needs the user to let them go. */
  undelivered_messages: number;
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
  kind: 'manager' | 'tech_lead' | 'worker' | 'reviewer';
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

export interface Snapshot {
  seq: number;
  project: Project | null;
  config: { version: number; config: ProjectConfig } | null;
  roles: RoleView[];
  tasks: TaskView[];
  attention: Attention[];
  quota: Domain[];
  harnesses: HarnessView[];
  live: Record<string, LiveTurn>;
}

/** A field moved to the blob store because it was large (data-model.md §7.3). */
export interface BlobRef {
  blob: string;
  size: number;
  head: string;
  tail: string;
}

/** A text field of an item; any of them goes to the blob store when it is large. */
export type Text = string | BlobRef;

/**
 * What an item holds, by kind (backend/crates/harness/src/event.rs `ItemKind`). `input` marks
 * where a turn's input was delivered; the messages it carried show on their own.
 */
export type ItemBody =
  | { kind: 'input'; content: { source: string } }
  | { kind: 'agent_message'; content: { text: Text } }
  | { kind: 'reasoning'; content: { text: Text } }
  | { kind: 'command'; content: { command: Text; output: Text | null; exit_code?: number | null; status?: string } }
  | { kind: 'file_change'; content: { changes: { path: string; kind: string }[] | null; status?: string } }
  | {
      kind: 'mcp_call';
      content: {
        server: string;
        tool: string;
        arguments: unknown;
        /** The first text the tool returned, and why the call failed; every adapter fills them. */
        result_text: string | null;
        error_text: string | null;
        /** As the harness gave them, for display only. */
        result?: unknown;
        error?: unknown;
        status?: string;
      };
    }
  | { kind: 'web_search'; content: { query: Text } }
  | { kind: 'todo_list'; content: { items: { text: string; completed: boolean }[] | null } }
  /** A tool of the harness's own that is neither a command nor a file change, such as reading a file. */
  | { kind: 'tool_call'; content: { tool: string; input: unknown; output: Text | null; status?: string } }
  | { kind: 'error'; content: { message: Text } }
  /** A kind the adapter does not know: the harness's raw item. */
  | { kind: 'other'; content: Record<string, unknown> };

export type Item = ItemBody & {
  id: string;
  seq: number;
  turn_id: string;
  /** For a call to a Lobotomy tool: the command it ran. */
  command_id: string | null;
  created_at: number;
};

/** An item of a running turn: shown while it runs, never stored (frontend.md §3). */
export type LiveItem = ItemBody & { started_at: number };

export interface LiveTurn {
  role: string;
  started_at: number;
  /** By the harness's item id. */
  items: Record<string, LiveItem>;
}

export interface Message {
  id: string;
  seq: number;
  role: string;
  /** The caller that sent it: `user`, `runtime` or `role:<name>`. */
  source: string;
  task_id: string | null;
  body: string;
  /** The turn it was delivered in, once bound. */
  turn_id: string | null;
  created_at: number;
}

export interface CommandRecord {
  name: string;
  args: Record<string, unknown>;
  result: unknown;
}

/** An executor's `org_report` (backend/crates/core/src/report.rs). */
export interface ReportArgs {
  title: string;
  body: string;
  status: 'progress' | 'blocked' | 'done';
  blocked_on: string | null;
}

/** What a report changed; `late` and `no_task` changed nothing. */
export type ReportEffect = 'recorded' | 'unchanged' | 'late' | 'no_task';

export interface ReportRecord {
  name: 'org_report';
  args: ReportArgs;
  result: ReportEffect;
}

export interface ThreadPage {
  items: Item[];
  has_more: boolean;
  turns: Record<string, Turn>;
  messages: Message[];
  commands: Record<string, CommandRecord>;
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

export interface CheckRun {
  id: string;
  verification_id: string;
  seq: number;
  command: string;
  exit_code: number | null;
  timed_out: boolean;
  output: { stdout?: Text; stderr?: Text };
  duration_ms: number;
}

export interface Attempt {
  id: string;
  task_id: string;
  seq: number;
  started_at: number;
  ended_at: number | null;
  end_reason: 'candidate' | 'abandoned' | null;
  /** Where its code starts; null when the slot stayed as it was. */
  code_start: string | null;
  done_turn_id: string | null;
  /** The title and body of that turn's done, for the candidate's commit message. */
  done_summary: string | null;
  /** An optional launch recipe from this round's done report; never inferred from its prose. */
  trial?: TrialRecipe | null;
  candidate_id: string | null;
  /** Files its code start left with conflict markers. */
  conflicts: string[];
}

export interface TrialRecipe {
  command: string;
  purpose: string;
}

/** Runtime-only desktop terminals; an open terminal does not mean the command succeeded. */
export interface TrialView extends TrialRecipe {
  id: string;
  task_id: string;
  attempt_id: string;
  round: number;
  candidate: string;
  directory: string;
  state: 'open' | 'closed';
  error: string | null;
}

export interface Decision {
  id: string;
  kind: string;
  actor: string;
  detail: Record<string, unknown>;
  created_at: number;
}

export interface TaskDetail {
  task: Task;
  criteria: { version: number; text: string; created_by: string; created_at: number }[];
  attempts: Attempt[];
  captures: Capture[];
  verifications: Verification[];
  checks: CheckRun[];
  decisions: Decision[];
  publications: { rev: number; commit_id: string; previous: string; created_at: number }[];
  /** The user's messages to the task that its executor never got (data-model.md §4.2). */
  undelivered_messages: Message[];
}

export interface RemoteError {
  code: string;
  message: string;
}

/** An entry of the global event log: `<object>.<what happened>`, such as `task.created`. */
export interface LogEvent {
  seq: number;
  kind: `${'attempt' | 'capture' | 'integration' | 'message' | 'native_session' | 'preview' | 'project' | 'report' | 'role' | 'task' | 'turn' | 'verification' | 'workspace'}.${string}`;
  entity: string;
  payload: { task_id?: string | null } & Record<string, unknown>;
}

export type Push =
  | { type: 'events'; events: LogEvent[] }
  | { type: 'live'; live: Record<string, LiveTurn> }
  | { type: 'thread'; role: string; seq: number }
  | { type: 'host' }
  | { type: 'tick' }
  | { type: 'resync' };
