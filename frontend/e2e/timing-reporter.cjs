// Optional diagnostic reporter: TEST_TIMING_FILE=/tmp/timing.json npx playwright test
// --reporter=./e2e/timing-reporter.cjs. Step durations overlap; use parents to avoid double counting.
const fs = require('node:fs');
const path = require('node:path');

function parents(step) {
  const result = [];
  for (let parent = step.parent; parent; parent = parent.parent) {
    result.push({ category: parent.category, title: parent.title });
  }
  return result;
}

module.exports = class {
  records = [];
  onStepEnd(test, _result, step) {
    this.records.push({ test: test.title, category: step.category, title: step.title, duration: step.duration, parents: parents(step) });
  }
  onTestEnd(test, result) {
    this.records.push({ test: test.title, category: 'total', duration: result.duration, status: result.status, errors: result.errors });
    console.log(`${result.status}: ${test.title} (${result.duration}ms)`);
    for (const error of result.errors) console.error(error.stack ?? error.message);
  }
  onEnd(result) {
    const output = process.env.TEST_TIMING_FILE ?? 'test-results/timing.json';
    fs.mkdirSync(path.dirname(output), { recursive: true });
    fs.writeFileSync(output, JSON.stringify({ status: result.status, duration: result.duration, records: this.records }, null, 2));
  }
};
