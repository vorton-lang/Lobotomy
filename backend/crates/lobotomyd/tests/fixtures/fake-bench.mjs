// Synthetic turns for the performance baseline (frontend.md §5), in Codex's event format. The
// content is the same on every run, so runs compare.
//
//   FAKE:many=<n>     n completed items of mixed kinds: replies with Markdown and code, commands
//                     with outputs of a few to a few hundred lines, reasoning, file changes, todos
//   FAKE:huge         one command with a 100,000-line output
//   FAKE:unclosed     a reply whose Markdown never closes: a code fence, emphasis, a list
//   FAKE:stream=<s>   a command that runs for s seconds and updates its output every 20 ms; each
//                     line carries the time it was written, so the GUI's lag can be measured
//   FAKE:marker=<w>   the turn's last reply is "bench <w>"

/** A small seeded generator: the same seed gives the same items. */
function random(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const words = 'slot capture verify accept preview thread turn item role task attempt store commit tree merge rebase check output index schema event sequence page snapshot push token'.split(' ');

function sentence(next, n) {
  return Array.from({ length: n }, () => words[Math.floor(next() * words.length)]).join(' ');
}

function reply(next, i) {
  const parts = [`### Step ${i}`, `${sentence(next, 18)}. ${sentence(next, 12)}.`];
  if (next() < 0.5) {
    parts.push('```rust', ...Array.from({ length: 4 + Math.floor(next() * 12) }, (_, j) => `    let value_${j} = store.read(&key_${j})?; // ${sentence(next, 4)}`), '```');
  }
  if (next() < 0.4) parts.push(...Array.from({ length: 3 }, () => `- **${sentence(next, 2)}**: ${sentence(next, 8)}`));
  if (next() < 0.15) parts.push('| file | lines |', '|---|---|', ...Array.from({ length: 4 }, (_, j) => `| src/${words[j]}.rs | ${Math.floor(next() * 900)} |`));
  return parts.join('\n');
}

function output(next) {
  const lines = next() < 0.8 ? 1 + Math.floor(next() * 30) : 100 + Math.floor(next() * 400);
  return Array.from({ length: lines }, (_, j) => `${String(j + 1).padStart(4)} | ${sentence(next, 6)}`).join('\n') + '\n';
}

function item(next, i, id) {
  const roll = next();
  if (roll < 0.35) return { id, type: 'agent_message', text: reply(next, i) };
  if (roll < 0.75) {
    const failed = next() < 0.1;
    return { id, type: 'command_execution', command: `cargo test -p ${words[i % words.length]}`, aggregated_output: output(next), exit_code: failed ? 101 : 0, status: failed ? 'failed' : 'completed' };
  }
  if (roll < 0.85) return { id, type: 'reasoning', text: `${sentence(next, 30)}.` };
  if (roll < 0.95) return { id, type: 'file_change', changes: [{ path: `src/${words[i % words.length]}.rs`, kind: 'update' }], status: 'completed' };
  return { id, type: 'todo_list', items: Array.from({ length: 5 }, (_, j) => ({ text: sentence(next, 5), completed: j < i % 5 })) };
}

/** Emits what the input asks for. */
export async function bench(input, emit) {
  const many = /FAKE:many=(\d+)/.exec(input);
  if (many) {
    const count = Number(many[1]);
    const next = random(count);
    for (let i = 0; i < count; i++) emit({ type: 'item.completed', item: item(next, i, `bench_${i}`) });
  }
  if (input.includes('FAKE:huge')) {
    const lines = Array.from({ length: 100_000 }, (_, j) => `line ${j} ${'x'.repeat(40)}`).join('\n');
    emit({ type: 'item.completed', item: { id: 'bench_huge', type: 'command_execution', command: 'bench huge output', aggregated_output: lines, exit_code: 0, status: 'completed' } });
  }
  if (input.includes('FAKE:unclosed')) {
    const text = '**An unclosed reply** with *emphasis that never ends\n\n- a list\n- that goes on\n\n```ts\nconst never = "closed";\nfunction open() {\n';
    emit({ type: 'item.completed', item: { id: 'bench_unclosed', type: 'agent_message', text } });
  }
  const stream = /FAKE:stream=(\d+)/.exec(input);
  if (stream) {
    const until = Date.now() + Number(stream[1]) * 1000;
    const lines = [];
    const base = { id: 'bench_stream', type: 'command_execution', command: 'bench stream', exit_code: null, status: 'in_progress' };
    emit({ type: 'item.started', item: { ...base, aggregated_output: '' } });
    for (let n = 0; Date.now() < until; n++) {
      lines.push(`tick ${n} at ${Date.now()}`);
      emit({ type: 'item.updated', item: { ...base, aggregated_output: lines.slice(-200).join('\n') } });
      await new Promise((r) => setTimeout(r, 20));
    }
    emit({ type: 'item.completed', item: { ...base, aggregated_output: lines.slice(-200).join('\n'), exit_code: 0, status: 'completed' } });
  }
  const marker = /FAKE:marker=(\S+)/.exec(input);
  if (marker) {
    emit({ type: 'item.completed', item: { id: 'bench_marker', type: 'agent_message', text: `bench ${marker[1]}` } });
  }
}
