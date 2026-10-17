# 给 agent 的约定

先读原则；做下面这些事时，过对应的检查单。检查单只列容易漏、漏了代价大的项；理由和历史在 notes/。

## 原则

1. **先保主路径。** 用户最常走的几条路坏了，先修它们；边角问题记下来，不抢主路径的注意力。
2. **事实、权威、判断分开。** 运行时持有事实，用户持有权威，Angela 与各 role 给出判断；判断不能冒充事实或权威。
3. **罕见的大风险靠设计和权限分工防。** 授权、CI 门槛、只引用原话。要验证就做演练，不靠推演和堆测试。
4. **恢复先靠手动，自动化要有证据。** 已知缺口写清谁会发现、人工怎么处理；真实出现过再考虑自动恢复。
5. **交还控制要显眼。** 自动化停下时，明确告诉用户现在由谁负责、要他做什么。
6. **不可逆的动作先核对对象和版本。** 例如按 SHA 合并，不合并"最新的"。
7. **测试追求杠杆，不追求召回率。** 本地测试用来尽早暴露设计问题，CI 是缺陷的兜底。
8. **短。** 规则、检查单和提示词只写致命项；历史和理由放 notes/。

## 检查单

### 加测试前

1. 它属于这三种之一吗？
   - 主路径还在运转；
   - 真实出现过的问题（在使用中、验收中或 CI 上）；
   - 同一类错误第二次出现（提交说明里引用上一次）。

   都不是就不加。推演出来的场景不加，包括安全场景：Lobotomy 是本机单用户工具，不防同机的恶意攻击者。快要重写的部分也不加。
2. 放在最便宜、能复现它的那一层了吗？顺序是 core 的契约测试 → 后端测试 → e2e。
3. e2e 只测前后端连起来才能看到的流程；界面行为用模拟的 WebSocket 测，参照 `e2e/ui.spec.ts`。

### 提交前

1. 按文末的表跑了受影响的测试吗？只改 notes/、注释或文档时不跑。
2. Rust：运行 `cargo fmt`，以及受影响 crate 的 `cargo clippy -p <crate> --all-targets -- -D warnings`。
3. 前端：在 `frontend/` 运行 `npm run format`。
4. 改了发给 GUI 的 Rust 类型（带 ts-rs 派生的结构）：运行 `npm run api:generate`，提交 `src/api/generated.ts`，再运行 `npm run api:check`。这个文件只由它生成，不手改。
5. 改了后端，又要跑 e2e：先运行 `cargo build -p lobotomyd`（e2e 用 debug 版后端）。
6. notes/ 中受这次改动影响的段落改了吗？只改触及的段落。

### 写文档时（notes/、README，以及 issue 评论和提交说明中的中文）

1. 同一概念用同一术语：区分 role、turn、attempt、native session、artifact；后端进程写"后端"。代码标识符保持原样。
2. 写明谁在什么条件下做什么。契约没定的列为待决，不替设计者补答案。
3. 一句一个主要意思，但保留连贯的短段落，不把每句都拆成列表。操作步骤编号，按执行顺序写。
4. 分开写行为要求、设计理由、实测结果及限制、未决问题。保留平台、版本、条件和约数；不把一次观察写成普遍保证。
5. 不借润色改变设计含义和约束强度。
6. 文档只写现在：现行的设计、理由和未决问题。什么时候改了什么交给 git，不写进文档。新决定与旧文冲突时，直接改写旧文，来由写进提交说明。谁定的要写明：用户定的标"用户定"，实现时的选择也标出来，免得后来的人把用户的决定当成可以随手推翻的细节。实测结果保留平台、版本和日期，这些是实测的条件，不是历史。
7. 只改本次触及的段落。

## 改什么跑什么

| 改动 | 本地运行 |
|---|---|
| `backend/crates/core` | `cargo test -p lobotomy-core` |
| `backend/crates/harness` | `cargo test -p lobotomy-harness` |
| `backend/crates/worker-protocol` | `cargo test -p lobotomy-worker-protocol` |
| `backend/crates/worker`（worker 宿主） | `cargo test -p lobotomy-worker` |
| `backend/crates/agent`（runner、执行者的 MCP 工具、恢复） | `cargo test -p lobotomy-worker --test runner`；改动也影响 Angela 时，加上下一行的 backend 测试 |
| `backend/crates/lobotomyd` 中的额度、调度与 Angela | `cargo test -p lobotomyd --test backend <名字片段>`，只运行名字含这个片段的测试，例如 `quota`、`angela` |
| `backend/crates/agent/src/launch.rs`、`capability.rs`（子进程的环境） | `cargo test -p lobotomyd --test environment` |
| `backend/crates/lobotomyd/src/workers.rs`（后端接 worker 宿主） | 先 `cargo build -p lobotomy-worker`，再 `cargo test -p lobotomyd --test workers` |
| `backend/crates/lobotomyd/src/gui` | `cargo test -p lobotomyd --test gui` |
| 发给 GUI 的 Rust 类型：带 ts-rs 派生的结构，在 core、agent 与 lobotomyd 中，例如 `core::view`、`agent::instance::LiveTurn`、`lobotomyd/src/gui` | 在 `frontend/` 运行 `npm run api:generate`，提交生成的 `src/api/generated.ts`；再运行 `npm run api:check` |
| `frontend/src` | `npm run lint`、`npm run typecheck` 和 `npm test`；改了界面行为时，加上相关的 e2e：`npx playwright test -g "<标题片段>"` |
| 对话列表的滚动与分页 | 再加上 CPU 降速的滚动测试：`npx playwright test -g "slow machine"` |
| `frontend/electron` | `npm run lint` 和 `npx playwright test e2e/electron.spec.ts` |
| 只改 `notes/`、注释或文档 | 不运行测试 |

- 改动跨多个 crate，或改了 core 中运行时依赖的命令时，加上 `lobotomyd` 中相关路径的测试。
- 改了共用的测试工具时，本地只跑用到改动部分的测试，整套交给 CI。共用的测试工具包括：假 CLI（`tests/fixtures/fake-*.mjs`）、`tests/common`、`e2e/fixtures.ts`、`e2e/mocked.ts`。
- 性能基线（`npm run bench`）只在 CI 上跑，只记录不设门槛；改了渲染或流式更新时，看 CI 的 bench 摘要。
