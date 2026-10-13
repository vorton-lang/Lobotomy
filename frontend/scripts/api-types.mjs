// Regenerate/check the Rust contract, then compile real serde output against its TS types.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const frontend = fileURLToPath(new URL('..', import.meta.url));
const check = process.argv.includes('--check');
const wire = execFileSync(
  'cargo',
  [
    'run',
    '--quiet',
    '--locked',
    '--manifest-path',
    '../backend/Cargo.toml',
    '-p',
    'lobotomyd',
    '--features',
    'ts-rs',
    '--example',
    'export-gui-types',
    ...(check ? ['--', '--check'] : []),
  ],
  { cwd: frontend, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] },
);

// Keep the serialized integer literal intact for tsc; independently verify JSON.parse's
// actual representation, including the existing precision loss beyond the safe integer range.
const parsed = JSON.parse(wire);
assert.equal(typeof parsed.pushes[0].events[0].seq, 'number');
assert.equal(Number.isSafeInteger(parsed.pushes[0].events[0].seq), false);
assert.notEqual(BigInt(parsed.pushes[0].events[0].seq), 9223372036854775807n);
assert.equal('reply_to' in parsed.message, false);
assert.equal('preview_stopped' in parsed.models.snapshot, false);

const directory = mkdtempSync(join(tmpdir(), 'lobotomy-api-'));
try {
  const source = join(directory, 'wire.ts');
  const types = resolve(frontend, 'src/api/generated').replaceAll('\\', '/');
  writeFileSync(
    source,
    `import type { SearchResult, Push, Snapshot, Refs, SendMessage } from ${JSON.stringify(types)};\n` +
      `(${wire.trim()} satisfies { search: SearchResult; pushes: Push[]; models: { snapshot: Snapshot; refs: Refs }; message: SendMessage });\n`,
  );
  execFileSync(
    process.execPath,
    [
      resolve(frontend, 'node_modules/typescript/bin/tsc'),
      '--noEmit',
      '--strict',
      '--skipLibCheck',
      '--target',
      'ES2023',
      '--module',
      'ESNext',
      '--moduleResolution',
      'bundler',
      source,
    ],
    { cwd: directory, stdio: 'inherit' },
  );
} finally {
  rmSync(directory, { recursive: true, force: true });
}
console.log(check ? 'GUI types and serde fixtures match.' : 'GUI types generated; serde fixtures match.');
