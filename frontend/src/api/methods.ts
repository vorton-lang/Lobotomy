// Only the scoping/idempotency conveniences are frontend-owned; params/results are generated.
import type { Commands, ReadMethods } from './generated';

export type ReadMethod = keyof ReadMethods;
export type ReadParams<M extends ReadMethod> = ReadMethods[M]['params'] & { project?: string };
export type ReadResult<M extends ReadMethod> = ReadMethods[M]['result'];
export type ReadArgs<M extends ReadMethod> =
  {} extends ReadParams<M> ? [params?: ReadParams<M>] : [params: ReadParams<M>];
export type CommandName = keyof Commands;
export type CommandParams<C extends CommandName> = Commands[C]['params'];
export type CommandResult<C extends CommandName> = Commands[C]['result'];
// Distribute over a union of command names, so each keeps its own params.
export type CommandArgs<C extends CommandName> = C extends CommandName ? Omit<CommandParams<C>, 'request_id'> : never;
export type CommandRequest<C extends CommandName> = { name: C; args: CommandParams<C>; project?: string };
