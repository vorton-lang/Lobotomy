# Worker 接口

本文是 [#86](https://github.com/vorton-lang/Lobotomy/issues/86) 技术方案（[tech-plan.md](tech-plan.md)）中 worker 部分的展开。它在 2026-10-16 作为草稿写成，2026-10-17 收进 notes/。已定和待决的事项列在 §12。它不是实现规格。依据：
- 2026-10-16 的两路资料调研：任务级 agent 协议；环境抽象与凭据隔离。
- 同日第 0 步实验，见 tech-plan.md §9。
- notes 中仍然适用的 turn、消息、问题语义（data-model.md §3）。

术语：
- **编排层**：Lobotomy 核心加 Angela。
- **worker**：接下一件交代完整的事、交回交付物的执行者。
- **agent**：worker 里实际干活的 CLI 会话。
- **环境**：worker 工作的地方，以及它对外的一切连接。
- **forge**：GitHub 这类代码托管平台。

## 0. 前提（用户确认，2026-10-16）

1. **worker 是 Lobotomy 的外部。** 要对外发布时，worker 层必须可替换。Lobotomy 为常见情况自带"电池"；小众用途由用户自己写适配；每个人可以为自己的工作环境开发 worker 配置。
2. **编排层只看 task 级接口。** bot、token、forge、CI、权限、端口、宿主路径都挡在接口内侧。
3. **worker = agent × 环境**，两部分各自抽象。
4. **审查返工**：续接 worker 原来的会话，追加消息。**需求变化**：停止当前 attempt，开新 attempt。
5. **CI 结果**由环境从权威来源读取，不采信 agent 的自述。

## 1. 分层与职责

```
编排层（Lobotomy 核心 + Angela）
   │  Worker 协议：跨进程，有版本，带能力声明
worker 宿主（自带电池或第三方实现）
   ├─ agent 驱动：启动、续接、中断 CLI 会话；解析事件；退出确认
   └─ 环境：工作区、凭据、出网、资源、交付、证据、采用、清理
```

| 谁 | 负责 | 看不到 |
|---|---|---|
| 编排层 | 决定做什么；交代任务；回答问题；审查；决定采用或放弃；调度并发和额度 | token、forge、CI 的细节、分支名、宿主路径、CLI 参数 |
| agent 驱动 | 把任务和消息变成 CLI 的 turn；投递回执；续接会话；中断与退出确认；能力裁剪；给 agent 提供汇报与提问的工具 | forge 凭据 |
| 环境 | 工作区的生命周期；凭据；agent 退出后的交付（推送、开 PR）；读取证据（CI）；按编排层的指令采用（合并）；资源上限与指标 | 任务为什么要做（只拿到执行所需的部分） |

- agent 面向的工具（汇报完成、报告受阻、提问）由 worker 宿主自己提供（例如一个小的 MCP 服务），不再直接连到 Lobotomy。Lobotomy 的 MCP 只留给 Angela。
- 现有的 harness adapter（`claude.rs`、`codex.rs`）、进程管理（Job Object、进程组）和 turn 语义，都迁进自带电池的 agent 驱动。

## 2. 对象模型

| 对象 | 身份 | 说明 |
|---|---|---|
| task | 编排层生成的 `task_id` | 交出去的一件事；带任务说明的版本号。对应 A2A 的 contextId |
| attempt | worker 生成，`(task_id, 序号)` | 按某个说明版本执行一次。进入终态后不再接收消息；新 attempt 指回上一个。对应 A2A 的 Task |
| message | 编排层生成的 `message_id` | 发给 attempt 的消息；回答问题时带 `question_id` |
| question | worker 生成的 `question_id` | agent 受阻时提出的问题，带可选项 |
| artifact | `artifact_id`，加上版本 | 交付物：`change`（代码，版本是 head SHA）、`document`（版本是内容哈希）、`report`（文本） |
| evidence | 绑定到 `(artifact_id, 版本)` | 一条检查或审查结果，标明来源：`environment` 或 `agent` |

turn 和 native session 留在 worker 内部。对外只以不透明 ID 出现在投递回执里。

## 3. 操作（编排层 → worker）

所有 ID 由调用方生成，同时作为幂等键：同一个 ID 重复调用，返回第一次的结果。

| 操作 | 作用 | 回执 |
|---|---|---|
| `hello` | 协商协议版本，交换能力声明（§8） | 能力清单 |
| `create_task(task_id, spec)` | 新建 task，开始 attempt 1 | `attempt_id` |
| `send_message(task_id, message_id, body, refs, question_id?)` | 给当前 attempt 发消息 | 只承诺"已持久保存并排队"。之后用事件报告绑定、投递或未投递 |
| `revise(task_id, spec_version, spec)` | 需求变化：在一步之内停止当前 attempt、开新 attempt，消除两步之间的竞态 | 新 `attempt_id`；退回还没投递的消息，标为"已被取代"。这些消息不转给新 attempt，由编排层决定哪些内容写进新的任务说明 |
| `stop(task_id, level)` | `level=turn`：只中断当前 turn，attempt 停在 delivered/blocked 等可继续的状态。`level=attempt`：取消 attempt | 写明作用到哪个 attempt；以进程确认退出为准 |
| `adopt(task_id, artifact_id, version)` | 编排层决定采用；由环境执行（代码就是合并） | `adopted`，或拒绝原因：版本已变、证据不足、冲突 |
| `close(task_id, outcome)` | `outcome`：adopted 或 abandoned。释放算力，按策略保留或清理工作区 | — |
| `subscribe(after_seq)` | 首帧给全部 task 的权威快照，之后补发 seq 之后的事件 | 事件流 |

- 任务说明无效，或要求了 worker 没有声明的能力时，`create_task` 当场返回错误，不创建 attempt。开始执行后的失败一律记为 `failed` 并写明原因。不设 `rejected` 状态。
- 可选能力：`read_log(task_id, attempt_id, cursor)` 按需读完整日志，供排查用。
- worker → 编排层只有一种请求：`put_blob(hash, chunk)`，按内容哈希分块上传非代码交付物，内容存进 Lobotomy 的文件库（§6）。
- 不设"暂停"原语。依据：实验和调研都显示，暂停在各家实现里都不可靠；需要时由编排层停止，之后再续接。

## 4. 状态（attempt 级）

```
submitted → preparing → working ⇄ blocked{needs_input | needs_auth, question_id}
                           ↓  ↑（审查意见：续接会话追加消息）
                        delivered（有交付物，证据满足 done_when，空闲等审查）
终态：adopted / abandoned / failed / canceled
非终态：unknown（失联或进程未确认退出，等待对账）
```

- 状态名借自 A2A；不同之处在于 `delivered` 不是终态。审查返工通过追加消息回到 `working`，这是用户确认的做法。
- `blocked` 和 `unknown` 是顶层状态，不藏在 running 的子字段里。依据：Devin 和 Cursor 把"等待输入"藏在子字段里，调用方因此误判，这是用户报告最多的问题。
- task 的状态 = 当前 attempt 的状态，再加上 `closed`。

## 5. 事件（worker → 编排层）

每个事件都带 `task_id`、`attempt_id` 和按 task 单调递增的 `seq`。

| 事件 | 内容 |
|---|---|
| `state_changed` | 新状态、原因 |
| `message_receipt` | `queued`、`bound(turn)`、`delivered`、`not_delivered(原因)` |
| `question_opened` / `question_closed` | 问题、选项；回答它的消息 |
| `progress` | 一行"最新动作"，供界面折叠显示；不唤醒 Angela。粒度和频率到实现时再定（§12） |
| `artifact_published` | `artifact_id`、版本、`supersedes`、显示信息（标题、PR 链接、改动规模） |
| `evidence_updated` | 绑定的 `(artifact_id, 版本)`、名称、`pending/passed/failed`、来源、链接 |
| `resources`（宿主级） | 运行中的 attempt、内存与磁盘、额度余量与重置时间（拿不到就标 unknown） |

## 6. 交付物与证据

- 交付物的每个新版本都是一次 `artifact_published`。版本变化时，旧证据保留为历史，不再支持当前版本。
- 只有 `source=environment` 的证据能满足 `done_when`。`source=agent` 的自述只作线索，在界面上标明。
- **CI 失败的内循环**（用户确认，2026-10-16）：done_when 写了"CI 通过"时，由环境把 CI 失败的摘要续接给 agent 去修，直到通过，或用完 `stop_when` 里的预算。编排层不会因为每次 CI 失败被唤醒，只看到 `delivered` 或 `failed`/`blocked`。
- `adopt` 是比较并交换：版本必须仍是当前版本，必需的证据必须在这个版本上通过，否则拒绝。
- 非代码交付物（document）由 worker 用 `put_blob` 按内容哈希分块上传，存进 Lobotomy 的文件库；交付物只记哈希。很小的报告直接内联。不接受 worker 本地的路径作为引用，理由：环境可能被清理，远端 worker 的路径 Lobotomy 也读不到（用户确认，2026-10-16）。

## 7. 凭据与权限（全部在环境内侧）

- **agent 不持有 forge 凭据。**
  - agent 的 clone 以环境自己的本地镜像为 origin。
  - 环境清掉 `GH_TOKEN` 这一类变量，设 `GIT_CONFIG_NOSYSTEM=1`，把凭据助手置空，并让 `GH_CONFIG_DIR` 指向空目录。
  - agent 的进程树确认退出后，环境把提交取回自己的仓库，再用 bot 身份推送、开 PR。
  - 环境不在 agent 的 clone 里带着 token 运行 git，因为那里的 git 配置和 hook 都由 agent 控制。
  - 依据：Copilot 的 agent 不能直接 push；Claude Code 云端的 token 不进 VM。第 0 步实验中，经 git 凭据助手发放的 token 让 agent 在技术上能合并自己的 PR。
- **防护等级。** 本地文件夹这一档只防失误，不防对抗：同一个 Windows 用户下，agent 仍能读到这个用户的凭据存储。更强的隔离要靠独立的本地账户、容器或远端。这与现有能力裁剪"防失误、不防对抗"的原则一致（harness-adapter.md §1.6）。
- **forge 端兜底**：main 的规则已要求走 PR 并通过 `ci-ok`。暂不另设合并身份（用户，2026-10-16）。
- **能力裁剪**（去掉用户的连接器和设置）继续由 agent 驱动负责。
- 超出本任务仓库的对外动作（向别人的仓库提 issue、发消息），worker 一律不提供，改走编排层的单独授权。

## 8. 能力声明与配置

`hello` 返回：
- 协议版本。
- agent：harness 和可选模型；是否支持续接；是否支持把消息插入运行中的 turn（steer），或只能排队；是否支持中断。
- 环境：支持的交付物种类、证据来源、隔离等级、平台、资源上限、是否支持 `adopt`。

编排层按声明行事，未声明的能力视为不支持。依据：A2A 和 ACP 都这样做。

扩展方式（用户确认，2026-10-16）：每个协议版本定一组核心能力；第三方扩展用带命名空间的键 `x-<作者>.<名字>`，不认识的一律忽略。不用 A2A 的 URI，因为那是给跨组织互联准备的，这里是一个人装几个插件。

**worker 配置**（自带电池用声明式配置，不用写代码）：

```toml
[worker.lobotomy-claude]
agent = { harness = "claude", model = "opus" }
environment = { kind = "local-folder", root = "...", memory_mb = 4096, ports = 10 }
source = { forge = "github", repo = "vorton-lang/Lobotomy", identity = "app:lobotomy-worker" }
delivery = { kind = "pull-request", base = "main" }
evidence = { required = ["ci-ok"] }
```

- 第三方 worker 实现同一协议即可。Lobotomy 提供一套**一致性测试**：幂等、断线补发、停止确认、证据绑定、迟到结果、能力声明。用户写的适配可以用它自检。

## 9. 幂等与恢复

- 调用方生成的 ID 就是幂等键。
- 事件按 task 用单调的 `seq` 编号，不用时间戳当游标。依据：OpenHands 按时间戳续传会漏事件。
- 重连时先取快照，再补发事件。以快照为准，不从事件重建状态。
- **worker 宿主重启**：仍有进程在跑、或退出未确认的 attempt 记为 `unknown`，确认进程退出后再继续或报告。这沿用 data-model.md §3.2–3.3 对 turn 的规定。
- **编排层重启**：重新订阅即可。
- **迟到的结果**：不是当前 attempt 的结果只记录，不推进状态。
- **不自动重投**：消息所在的 turn 没有正常结束时，消息不自动重投（data-model.md §3.4）。
- `stop` 对已在终态的 attempt 无副作用，返回当前状态。`adopt` 按 `(artifact_id, 版本)` 幂等。

## 10. 传输

- 本地：worker 宿主是 Lobotomy 启动的子进程，用 stdio 上的 JSON-RPC 通信，与 LSP、MCP 相同。远端：同样的消息走 WebSocket。
- 自带电池从一开始就跑在独立进程里，不走进程内的特权通道（用户确认，2026-10-16）。这样协议能不能撑起完整功能，第一天就会暴露出来；worker 崩溃也不会拖垮 Lobotomy。代价是多一层进程管理，这部分现在已经有了。
- 消息类型用 Rust 定义，像现在的 GUI 类型一样生成 TypeScript 和 JSON Schema，给第三方用。

## 11. 与现有实现的对应

| 现有 | 去向 |
|---|---|
| `harness` crate（claude、codex、process） | 自带电池的 agent 驱动 |
| `core` 中的 turn、message、question 语义 | 迁进 worker 宿主内部；对外以 task 级状态和投递回执出现 |
| MCP 中 role 的工具（`org_report` 等） | 改由 worker 宿主提供给自己的 agent |
| `store` crate、采集、验证、预览、展示分支采用 | 退出，已于 2026-10-17 删除（tech-plan.md §6）；由环境的交付、证据和采用取代 |
| 额度域（quota） | 分两处：worker 报告额度余量，编排层负责调度 |

## 12. 已定与待决

**已定（用户，2026-10-16）**
- **CI 失败的内循环**：由环境在 `stop_when` 的预算内自动交回 agent 修，编排层不因每次 CI 失败被唤醒（§6）。
- **`adopt` 的执行身份**：先沿用同一个 bot，不另设身份。worker 能否自己合并，靠 §7 的"agent 不持有 forge 凭据"来挡。

- **`revise` 时还没投递的消息**：不转给新 attempt，在回执里退回编排层，标为"已被取代"（§3）。
- **不设 `rejected` 状态**：无效或不支持的任务由 `create_task` 当场报错；执行后的失败记为 `failed`（§3）。
- **传输**：自带电池从一开始就跑在独立进程里（§10）。
- **非代码交付物**：用 `put_blob` 按内容哈希分块上传，小报告内联，不接受 worker 本地路径（§6）。
- **能力扩展**：核心能力跟着协议版本走；扩展用 `x-<作者>.<名字>`，不认识的忽略（§8）。

**待决**
- **`progress` 的粒度和频率**：暂时没有想法，做到时再考察（用户，2026-10-16）。
