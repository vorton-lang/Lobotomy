// The latest baseline run as a Markdown table, for the CI job's summary.
import fs from 'node:fs';

const run = JSON.parse(fs.readFileSync('bench-results/latest.json', 'utf8'));
const rows = [];
const walk = (name, value) => {
  if (value && typeof value === 'object') for (const [key, inner] of Object.entries(value)) walk(name ? `${name}.${key}` : key, inner);
  else rows.push(`| ${name} | ${value} |`);
};
walk('', run.results);
const { os, cpu, cores, memory_gb } = run.machine;
console.log(
  [
    `### 性能基线 · ${os}`,
    '',
    `${cpu}，${cores} 线程，${memory_gb} GB；浏览器 ${run.browser}；提交 ${run.commit}；${run.items} 个 item。`,
    '只记录，不设门槛（notes/perf-baseline.md）。',
    '',
    '| 指标 | 值 |',
    '|---|---|',
    ...rows,
  ].join('\n'),
);
