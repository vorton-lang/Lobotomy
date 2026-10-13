# Frontend checks

Run from `frontend/` after `npm ci`:

```sh
npm run format:check
npm run lint
npm run typecheck
npm run api:check
npm test
npm run build
```

CI runs these checks and Playwright on Windows and Ubuntu for every PR, including
PRs targeting another branch. Benchmarks still run only outside PRs.

## Static checks

Oxlint is the only linter. Its core, TypeScript and Oxc correctness rules cover
`src/`, Electron main/preload, scripts, Vite/Playwright configuration, e2e and
bench sources. It parses TypeScript independently of the compiler package;
typescript-eslint currently requires TypeScript `<6.1`, while this project uses
TypeScript 7. The dependency is pinned so rule changes are reviewed on upgrade.
No style rules or React Compiler migration rules are enabled.

TypeScript owns strict typing and unused TS declarations. Oxlint's duplicate
`no-unused-vars` check is disabled only for TS/TSX. JavaScript keeps that rule and
adds `no-undef`, with Node globals for the main process and tooling, and Node plus
browser globals for Electron's preload. Inline exclusions must explain the
specific constraint; unused exclusions fail lint.

`npm run typecheck` runs three explicit environments, sharing the original strict,
unused-declaration and no-emit options in `tsconfig.base.json`:

- `typecheck:app`: renderer and worker sources with the existing browser/worker
  libraries and Vite types, including the compile-only API regression checks.
- `typecheck:node`: Vite, Playwright and benchmark configuration/setup with Node
  types and no DOM library.
- `typecheck:tests`: Vitest, e2e and benchmark TS sources with Node and DOM types.
  Playwright tests contain both Node test code and callbacks executed in the page.

Electron's JavaScript is linted, not converted to TypeScript. Generated
`src/api/generated.ts` is excluded from lint: Rust's exporter owns its text, and
`api:check` checks exact drift and real serde fixtures. It remains typechecked
through the app and tests. Do not hand-edit it.

For local behavioral tests, follow the scope table in `../AGENTS.md`. In
particular, Electron changes require `npx playwright test e2e/electron.spec.ts`,
with a freshly built debug backend. Benchmarks are not run locally.

## Formatting

`npm run format` writes Prettier formatting; `npm run format:check` checks it in CI.
Both are limited to this frontend directory. Prettier is pinned and handles
TS/TSX, JS/MJS/CJS, CSS, HTML, JSON and this README, without lint rules or plugins.
The configuration keeps single quotes, semicolons, two-space indentation and LF,
with a 120-column print width. HTML whitespace is treated as significant and
embedded languages in strings are not reformatted.

`src/api/generated.ts` and `package-lock.json` are excluded because Rust and npm
own their output. Build, dependency, local scratch and test-result directories
are also excluded. After API changes use `npm run api:generate`, then
`npm run api:check`; do not run a formatter over the generated contract.
