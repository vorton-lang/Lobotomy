// Wire DTOs come from Rust. Dynamic content stays JsonValue until a display helper reads it.
export type * from './generated';
import type { HarnessView, ReportArgs, ReportEffect } from './generated';

/** Display choices and interpretations of dynamic JSON, not additional wire DTOs. */
export type Permission = HarnessView['permission'];
export type ReportRecord = { name: 'org_report'; args: ReportArgs; result: ReportEffect };
