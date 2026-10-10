# 给 agent 的约定

## 文档写作

本节适用于 `notes/`、`README.md`，以及 issue 评论和提交说明中的中文文字。它借鉴 ASD-STE100 的清晰表达原则（[#8](https://github.com/vorton-lang/Lobotomy/issues/8)），不宣称符合 STE，也不要求改写成英文。

只改本次触及的段落，不全仓重写。

1. **同一概念使用同一术语。** 区分 role、turn、attempt、native session、artifact。代码标识符保持原样。不为避免重复而换用同义词，也不强行翻译。例如，Lobotomy 的后端进程统一写"后端"，不写"守护进程"。
2. **写明谁在什么条件下做什么。** 用明确的主体和动作。用到"处理""恢复""完成"这类词时，写明已知的对象和边界。契约还没定时，列为待决问题，不替设计者补答案。
3. **一句表达一个主要意思。** 拆开密集的分号和层叠的从句，但保留连贯的短段落，不把每个分句都变成列表。操作步骤用编号，按执行顺序写。
4. **分开写四类内容：** 行为要求、设计理由、实测结果及限制、未决问题。保留平台、版本、条件和约数。不把一次观察写成普遍保证。
5. **保持设计含义、约束强度和不变量。** "建议"仍是建议，"假定"仍是前提，"待验证"仍需验证。不借润色新增要求、恢复机制，或改变业务语义。新决定与旧文冲突时，单独标出"**语义更新**"，并写明依据，例如 issue 编号、用户确认和日期。

改完后检查：

- 术语是否一致？
- 主体、条件和动作是否明确？
- 步骤是否有序？
- 要求、理由、证据和问题是否分清？
- 是否无意改变了设计含义？

## 测试：改什么跑什么

### 测试的目的

本地测试的目的不是拦住所有 bug，而是尽早暴露设计问题，减少 bug 带来的开发方向偏移。CI 负责完整的重量级测试，也是缺陷的兜底。

- 要尽早拦住的：暴露设计缺口的问题。例如有一类情况没考虑到，修好它要长出一大截结构。
- 可以放过的：只在某些情况下行为不对、修起来只是局部改动的 bug。由 CI 发现，之后再修。

写测试时：

- 优先写快速的契约测试：命令的前置条件、状态迁移、不变量。lobotomy-core 的测试一个只要零点几秒，最能提示设计问题。
- e2e 只测前后端连起来才能看到的流程。界面行为用模拟的 WebSocket 测，参照 `e2e/markdown.spec.ts`；不要为了检查一张卡片跑完整条流水线。
- 后端或 core 已经覆盖的逻辑，不再用 e2e 重测。

### 流程

全套测试很慢，主要时间花在等待进程和 I/O 上。所以本地只运行受改动影响的测试。推送到 main 后，CI 在 Windows 和 Ubuntu 上运行全套：格式、后端测试、Clippy、前端类型检查与单元测试、构建、e2e。

1. 本地运行下表中受影响的测试。
2. 自己的改动通过后直接推送到 main，不开 PR，也不等 PR 的 CI。合进 main 后，CI 会把同样的测试再跑一遍。
3. 不要停下来等 CI。把等待放到后台，例如 `gh run watch <run id> --exit-status`，先做下一件事；没有下一件事时，先汇报 CI 正在运行。
4. main 上 CI 失败时，不用自己修。外部流程每小时检查 main 上新的 CI 失败：普通问题修好后开 PR，不简单的问题开 issue。

agent 负责验收并合并这些修复 PR：看改动是否只修了失败的原因、是否符合 notes/ 中的设计，以及 PR 的 CI 是否通过；验收后 squash 合并。外部流程开的 issue 按设计问题对待。开始工作时，先看有没有待验收的 PR。

审查别人的分支（例如另一个 agent 的集成分支）时也走 PR 合入 main，CI 只跑这一轮。

| 改动 | 本地运行 |
|---|---|
| `backend/crates/core` | `cargo test -p lobotomy-core` |
| `backend/crates/store` | `cargo test -p lobotomy-store`，再加 `cargo test -p lobotomyd --test results` |
| `backend/crates/harness` | `cargo test -p lobotomy-harness` |
| `backend/crates/lobotomyd` 的运行时（runner、MCP、采集、验证、额度、恢复） | `cargo test -p lobotomyd --test backend <名字片段>`，只运行名字含这个片段的测试，例如 `quota`、`claude`、`check` |
| `backend/crates/lobotomyd/src/launch.rs`（子进程的环境） | `cargo test -p lobotomyd --test environment` |
| `backend/crates/lobotomyd/src/gui` | `cargo test -p lobotomyd --test gui` |
| `frontend/src` | `npm run typecheck` 和 `npm test`；改了界面行为时，加上相关的 e2e：`npx playwright test -g "<标题片段>"` |
| 对话列表的滚动与分页 | 再加上 CPU 降速的滚动测试：`npx playwright test -g "slow machine"` |
| `frontend/electron` | `npx playwright test e2e/electron.spec.ts` |
| 只改 `notes/`、注释或文档 | 不运行测试 |

- 改动跨多个 crate，或改了 core 中运行时依赖的命令时，加上 `lobotomyd` 中相关路径的测试。
- 改了共用的测试工具时，本地只运行用到改动部分的测试，整套交给 CI。共用的测试工具包括：假 CLI（`tests/fixtures/fake-*.mjs`）、`tests/common`、`e2e/fixtures.ts`、`e2e/mocked.ts`。
- e2e 使用 debug 版后端。改了后端之后，先运行 `cargo build -p lobotomyd`，再运行 e2e。
- 改了 Rust 代码后，提交前运行 `cargo fmt`，以及受影响 crate 的 `cargo clippy -p <crate> --all-targets -- -D warnings`。
- 默认忽略的真实 CLI 测试会消耗订阅额度。只在改了 adapter 的参数、事件解析或 MCP 接口时运行，例如 `cargo test -p lobotomyd --test backend a_real_claude -- --ignored`。MCP 契约测试在 `--test mcp_contract` 中，也默认忽略。
- 性能基线（`npm run bench`）每次推送到 main 时由 CI 运行，只记录，不设门槛。本地不运行；改了渲染或流式更新时，看 CI 的 bench 摘要。
