# Worker 接口

本文是 [#86](https://github.com/vorton-lang/Lobotomy/issues/86) 技术方案（[tech-plan.md](tech-plan.md)）中 worker 部分的展开。已定和待决的事项列在 §12。它不是实现规格。依据：
- 2026-10-16 的两路资料调研：任务级 agent 协议；环境抽象与凭据隔离。
- 同日第 0 步实验，见 tech-plan.md §9。
- notes 中仍然适用的 turn、消息、问题语义（data-model.md §3）。

术语：
- **编排层**：Lobotomy 核心加 Angela。
- **worker**：接下一件交代完整的事、交回交付物的执行者。
- **agent**：worker 里实际干活的 CLI 会话。
- **环境**：worker 工作的地方，以及它对外的一切连接。
- **forge**：GitHub 这类代码托管平台。

## 0. 前提（用户定）

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

- agent 面向的工具（汇报完成、报告受阻、提问）由 worker 宿主自己提供（例如一个小的 MCP 服务），不直接连到 Lobotomy。Lobotomy 的 MCP 只给 Angela 用。
- 自带电池的 agent 驱动使用 harness adapter（`claude.rs`、`codex.rs`）、进程管理（Job Object、进程组）和 turn 语义。后端运行 Angela 用的是同一个 agent 驱动（§11）。

## 2. 对象模型

| 对象 | 身份 | 说明 |
|---|---|---|
| task | 编排层生成的 `task_id` | 交出去的一件事；带任务说明的版本号。对应 A2A 的 contextId |
| attempt | worker 生成，`(task_id, 序号)` | 按某个说明版本执行一次。进入终态后不再接收消息；新 attempt 指回上一个。对应 A2A 的 Task |
| message | 编排层生成的 `message_id` | 发给 attempt 的消息；回答问题时带 `question_id`，续接一次停下时带 `stop_id` |
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
| `send_message(task_id, message_id, body, refs, question_id?, stop_id?)` | 给当前 attempt 发消息 | 只承诺"已持久保存并排队"。之后用事件报告绑定、投递或未投递 |
| `revise(task_id, spec_version, spec)` | 需求变化：在一步之内停止当前 attempt、开新 attempt，消除两步之间的竞态 | 新 `attempt_id`；退回还没投递的消息，标为"已被取代"。这些消息不转给新 attempt，由编排层决定哪些内容写进新的任务说明 |
| `stop(task_id, level)` | `level=turn`：只中断当前 turn，attempt 停在 delivered/blocked 等可继续的状态。`level=attempt`：取消 attempt | 写明作用到哪个 attempt；以进程确认退出为准 |
| `adopt(task_id, artifact_id, version)` | 编排层决定采用；由环境执行（代码就是合并） | `adopted`，或拒绝原因：版本已变、证据不足、冲突 |
| `close(task_id, outcome)` | `outcome`：adopted 或 abandoned。释放算力，按策略保留或清理工作区 | — |
| `subscribe(after_seq)` | 首帧给全部 task 的权威快照，之后补发 seq 之后的事件 | 事件流 |

- 任务说明无效，或要求了 worker 没有声明的能力时，`create_task` 当场返回错误，不创建 attempt。开始执行后，attempt 级的失败记为 `failed` 并写明原因，例如预算用完、环境准备不了。不设 `rejected` 状态。
  - turn 异常结束不算 attempt 失败（用户定）。turn 被 `stop(turn)` 打断、CLI 失败或额度被拒时，attempt 停在 `blocked`，`need` 为 `resume`，写明原因，不带问题。编排层用写明这次停下的 `send_message` 续接；worker 宿主沿用"继续"的语义起新 turn：同一会话、同一工作区，harness 没开始过的 turn 原样重发（data-model.md §3.4、§9.2）。运行时不自动续接，额度失败也一样。
  - 续接是对一次停下的回复（用户定）。每次停下带一个 ID，像问题一样；`send_message` 写明它回复哪一次停下。第一条回复关掉这次停下，之后对同一 ID 的回复被拒绝，并说明谁在什么时候已经回复。不写回复对象的消息是普通消息，排到下一个 turn。依据：实际验收中，用户和 Angela 几乎同时续接，后到的一条被当成普通消息排进了下一个 turn。
    - 实现：停下的 ID（`blocked.stop_id`）取造成这次停下的那个 turn 的 ID（实现时的选择）。worker 宿主逐条接收消息；回复一次已经有人回复过的停下，拒绝原因是 `stop_answered`，写明那条消息和时间；回复一次 attempt 已不在的停下，拒绝原因是 `not_stopped`。
    - 编排层在消息写进项目库时就判定：同一次停下已有回复时，后到的一条不写入，直接拒绝，写明谁在什么时候回复的。消息回答的问题和续接的停下也在这时定下，重发同一条消息时参数不变。
- 过程记录由 worker 推给编排层保存，task 目录用完即删，不放持久内容（用户定）。agent 每完成一个条目，worker 带 seq 推送；`close` 带上编排层已存到的 seq，worker 确认没有更晚的条目后才删目录。这样 worker 搬到远端后，查看过程也不必经过网络去问 worker，因此不需要按需读完整日志的 `read_log`。推送和 `close` 时的 seq 确认未实现，随 tech-plan.md §7 第 3 步"查看过程"做。
- worker → 编排层只有一种请求：`put_blob(hash, chunk)`，按内容哈希分块上传非代码交付物，内容存进 Lobotomy 的文件库（§6）。
- 不设"暂停"原语。依据：实验和调研都显示，暂停在各家实现里都不可靠；需要时由编排层停止，之后再续接。

## 4. 状态（attempt 级）

```
submitted → preparing → working ⇄ blocked{needs_input | needs_auth | resume, question_id?, stop_id?}
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
| `mismatch_flagged` | agent 认为编排层的解读与用户原话不一致，写明哪里不一致、按哪个理解在做（现有的 `flag_mismatch`，manager-actions.md §2.3）。编排层直接显示给用户，不经 Angela 转述 |
| `resources`（宿主级） | 运行中的 attempt、内存与磁盘、额度余量与重置时间（拿不到就标 unknown） |

## 6. 交付物与证据

- 交付物的每个新版本都是一次 `artifact_published`。版本变化时，旧证据保留为历史，不再支持当前版本。
- 只有 `source=environment` 的证据能满足 `done_when`。`source=agent` 的自述只作线索，在界面上标明。
- **CI 失败的内循环**（用户定）：done_when 写了"CI 通过"时，由环境把 CI 失败的摘要续接给 agent 去修，直到通过，或用完 `stop_when` 里的预算。编排层不会因为每次 CI 失败被唤醒，只看到 `delivered` 或 `failed`/`blocked`。
- `adopt` 是比较并交换：版本必须仍是当前版本，必需的证据必须在这个版本上通过，否则拒绝。
- 非代码交付物（document）由 worker 用 `put_blob` 按内容哈希分块上传，存进 Lobotomy 的文件库；交付物只记哈希。很小的报告直接内联。不接受 worker 本地的路径作为引用，理由：环境可能被清理，远端 worker 的路径 Lobotomy 也读不到（用户定）。

## 7. 凭据与权限（全部在环境内侧）

- **agent 不持有 forge 凭据。**
  - agent 的 clone 以环境自己的本地镜像为 origin。
  - 环境清掉 `GH_TOKEN` 这一类变量，设 `GIT_CONFIG_NOSYSTEM=1`，把凭据助手置空，并让 `GH_CONFIG_DIR` 指向空目录。
  - agent 的进程树确认退出后，环境把提交取回自己的仓库，再用 bot 身份推送、开 PR。
  - 环境不在 agent 的 clone 里带着 token 运行 git，因为那里的 git 配置和 hook 都由 agent 控制。
  - 依据：Copilot 的 agent 不能直接 push；Claude Code 云端的 token 不进 VM。第 0 步实验中，经 git 凭据助手发放的 token 让 agent 在技术上能合并自己的 PR。
- **防护等级。** 本地文件夹这一档只防失误，不防对抗：同一个 Windows 用户下，agent 仍能读到这个用户的凭据存储。更强的隔离要靠独立的本地账户、容器或远端。这与现有能力裁剪"防失误、不防对抗"的原则一致（harness-adapter.md §1.6）。
- **forge 端兜底**：main 的规则要求走 PR 并通过 `ci-ok`。暂不另设合并身份（用户定）。
- **能力裁剪**（去掉用户的连接器和设置）由 agent 驱动负责。
- 超出本任务仓库的对外动作（向别人的仓库提 issue、发消息），worker 一律不提供，改走编排层的单独授权。

## 8. 能力声明与配置

`hello` 返回：
- 协议版本。
- agent：harness 和可选模型；是否支持续接；是否支持把消息插入运行中的 turn（steer），或只能排队；是否支持中断。
- 环境：支持的交付物种类、证据来源、隔离等级、平台、资源上限、是否支持 `adopt`。

编排层按声明行事，未声明的能力视为不支持。依据：A2A 和 ACP 都这样做。

扩展方式（用户定）：每个协议版本定一组核心能力；第三方扩展用带命名空间的键 `x-<作者>.<名字>`，不认识的一律忽略。不用 A2A 的 URI，因为那是给跨组织互联准备的，这里是一个人装几个插件。

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
- 自带电池跑在独立进程里，不走进程内的特权通道（用户定）。这样协议能不能撑起完整功能，会尽早暴露出来；worker 崩溃也不会拖垮 Lobotomy。代价是多一层进程管理，这部分现在已经有了。
- 消息类型用 Rust 定义，像现在的 GUI 类型一样生成 TypeScript 和 JSON Schema，给第三方用。

**协议 0.1 的实现**（[worker-protocol](../backend/crates/worker-protocol)）。下面几项是实现时的选择，用户没有逐条确认：
- 帧格式：stdio 上每行一条 JSON-RPC 2.0 消息，与 MCP 的 stdio 传输相同。一行解析不了，不影响后面的行。
- `subscribe` 的参数是按 task 的"已收到的最后一个 seq"。worker 宿主先回快照，再补发这些 task 之后的、它还留着的事件，然后接着推送新事件。
- 任务说明里的预算写成机器可读的字段：`turns`（attempt 的 turn 数，含 CI 返工）和 `ci_rounds`（CI 失败交回 agent 的次数）。`stop_when` 的文字仍然交给 agent。必需的检查单列为 `required_checks`，例如 `ci-ok`。
- 协议自己的拒绝用错误码 -32000，原因写在 `data.reason`，例如 `unknown_task`、`invalid_spec`。
- 未实现：生成 TypeScript 与 JSON Schema（现在只有 Rust 的两端用它）；`put_blob`。

**自带的 worker 宿主**（[worker](../backend/crates/worker)）。可执行文件是 `lobotomy-worker`，由编排层启动，用 stdio 通信，日志写 stderr。做法如下：
- 每个 task 一个实例目录，里面是一个项目结构的数据库，agent 是其中唯一的 role。每个 attempt 是这个库里的一个任务。turn、消息、问题沿用 data-model.md §3、§4 的规则，由共用的 agent 驱动运行。宿主自己的 `worker.db` 记录 task、attempt、消息的对应关系和发出的事件。
- attempt 的状态由实例中任务的阶段推出：执行中为 `working`；有开放问题为 `blocked`（`needs_input`）；上一个 turn 异常结束为 `blocked`（`resume`）；未结束的 turn 处于 unknown 为 `unknown`；等验收为 `delivered`。
- `send_message` 按状态落到实例里：`delivered` 时是退回返工，`resume` 时写明这次停下的消息是继续，其余是排队给下一个 turn。
- `stop` 和 `revise` 先打断进程；等 attempt 没有未结束的 turn，才把它记为 `canceled`。`close(abandoned)` 同理，记为 `abandoned`。
- CLI 不响应打断时，宿主在宽限期后连同进程树结束它，这个 turn 记为被打断。宽限期由参数 `--stop-grace <秒>` 给出，默认 10 秒。这是实现时的选择，用户没有逐条确认。
- 编排层关闭 stdin 时，宿主退出：不先打断，直接结束正在运行的 CLI 的进程树。这些 turn 在宿主下次启动时核对（harness-adapter.md §1.8，用户定）。
- 额度：turn 被拒时，宿主发 `resources` 通知，自己不封住额度域，也不自动续接；之后同一 harness 上有 turn 正常结束，就报告额度已恢复。编排层收到被拒的通知后，封住本机的这个额度域：所有项目都不再向这个 harness 派任务，直到用户重试（data-model.md §8.4，tech-plan.md §7 第 1 步的范围）。

**空闲时也停在 `blocked`**（用户定）：turn 正常结束，但 agent 既没有汇报 done 也没有提问，而且没有排队的消息时，attempt 停在 `blocked`（`resume`），原因写明。理由：此时什么都不在运行，报 `working` 会把等待藏起来（§4）。会出现的情况：汇报的工具调用失败（#13 记录过一次被拒）；模型只在文字里说"做完了"或提问；只报了 `progress`；模型把命令放到后台后结束了 turn，而 print 模式下后台任务结束不会再唤醒它（harness-adapter.md §1.6 的实测）。

**本地文件夹环境加 git**。启动时给了 `--upstream`，环境就按下面的方式工作；没给时，每个 task 用一个普通文件夹。
- 每个 worker 宿主一个上游仓库的 bare 镜像。每个 attempt 准备时（状态 `preparing`），先从上游更新镜像，再从镜像 clone 一份，在分支 `lobotomy/<task>-<序号>` 上从基线开始。clone 的 origin 是镜像。
- agent 的进程得到 L1 的变量：
  - 自己的 `TMP`、`TEMP`、`TMPDIR`。
  - 一份 attempt 专用的 git 配置（`GIT_CONFIG_GLOBAL`），加上 `GIT_CONFIG_NOSYSTEM=1`。这份配置里没有凭据助手，`remote.origin.pushurl` 指向一个不存在的地址，换行符按仓库原样，提交身份由宿主的参数给出。
  - `LOBOTOMY_PORT_BASE`：从 42000 起，每个运行中的 attempt 分 10 个端口。
- 环境在 clone 上运行的 git 命令用同一份配置，不运行 clone 里的 hook。
- 交付：attempt 进入 `delivered` 时，agent 的进程树已经退出。环境把分支取回镜像，存在 `refs/lobotomy/<task>-<序号>` 下，发布 `change` 交付物（版本是 head SHA，附改动规模）；同时把 agent 的 done 说明发布为 `report` 交付物。审查返工后再交付，同一个交付物出新版本；没有变化的不再发布。
- **实现时的选择**：agent 交付时如果留有未提交的改动，环境替它提交一次，提交说明写明"交付时由环境提交"，交付物标题也注明。理由：成果按工作目录的最终状态算，不让改动丢在 clone 里。
- 准备或取回失败时，attempt 记为 `failed`，原因写明。

**GitHub forge**。启动时给了 `--forge github --repo <owner/name>`，环境就按下面的方式工作。
- 凭据：worker 宿主以 GitHub App 的身份签发 token，限定在这个仓库，有效一小时，提前十分钟换新；也可以用环境变量 `LOBOTOMY_FORGE_TOKEN` 直接给 token，测试就用这种方式。宿主读到这个变量后，立即从自己的环境中删掉，免得 agent 的进程继承。git 通过环境变量里的 `http.extraHeader` 拿到 token，token 不出现在命令行参数和 URL 中。
- 允许开 PR 的任务交付时，环境从镜像把分支推到上游，再开 PR；如果这个分支已经有开着的 PR，就沿用。`change` 交付物的链接就是这个 PR。
- 证据：读取交付版本上的 check run，每个 check 一条来源为 `environment` 的证据。有检查还没出结果时，每 20 秒读一次。
- 状态：必需的检查都通过了，才是 `delivered`；还在等时是 `working`，原因写明在等哪些检查。
- CI 内循环：必需的检查失败时，环境把失败的检查和失败 job 日志的末尾（最多 80 行）作为退回意见交给 agent，在同一会话里返工。这件事每个版本只做一次。`ci_rounds` 用完后，attempt 记为 `failed`。`turns` 预算用完后也记为 `failed`。
- `adopt`：版本必须仍是当前版本，必需的检查必须在这个版本上通过，然后用 GitHub 的合并接口做 squash 合并，并指定 head SHA。PR 的 head 变了（409）记为 `version_changed`，GitHub 拒绝合并记为 `conflict`。合并成功后，删掉远端分支。重复调用 `adopt` 返回同一个结果。
- attempt 被停止或放弃时，关掉它的 PR。

实测（2026-10-17，真实的 vorton-lang/Lobotomy，只读）：App 签发 token、读取 check run、取失败 job 的日志末尾都正常。日志端点会跳转到下载地址，跳转能正常跟随。这次实测没有覆盖开 PR、推送和合并；它们在实际验收中走通，见 tech-plan.md §7"实际验收"。

工作区清理：任务关闭（`adopted` 或 `abandoned`）且所有 attempt 收尾完成后，宿主等待 runner 和实例引用释放，关闭数据库并释放 lock，再删除整个 `tasks/<task_id>/` 目录。启动恢复时也清理已结束任务的遗留目录，未结束任务继续恢复。目录不存在视为成功；删除失败有限次重试后只记 warning，不中止宿主。共享 bare 镜像及其中的引用保留。

还没做的：
- L1 的内存上限。
- `progress` 事件：粒度待决，见 §12。

## 11. 与现有实现的对应

| 现有 | 去向 |
|---|---|
| `harness` crate（claude、codex、process） | 自带电池的 agent 驱动 |
| `core` 中的 turn、message、question 语义 | 共用：Angela 也用这套语义，所以 agent 驱动是共用的 crate，后端和 worker 宿主各用一份（tech-plan.md §7，用户定）。在 worker 宿主中，它们对外以 task 级状态和投递回执出现 |
| MCP 中 role 的工具（`org_report` 等） | 由 worker 宿主提供给自己的 agent |
| `store` crate、采集、验证、预览、展示分支采用 | 已删除（tech-plan.md §6）；由环境的交付、证据和采用取代 |
| 额度域（quota） | 分两处：worker 报告额度余量，编排层负责调度 |

## 12. 已定与待决

**已定（用户定）**
- **CI 失败的内循环**：由环境在 `stop_when` 的预算内自动交回 agent 修，编排层不因每次 CI 失败被唤醒（§6）。
- **`adopt` 的执行身份**：先沿用同一个 bot，不另设身份。worker 能否自己合并，靠 §7 的"agent 不持有 forge 凭据"来挡。

- **`revise` 时还没投递的消息**：不转给新 attempt，在回执里退回编排层，标为"已被取代"（§3）。
- **不设 `rejected` 状态**：无效或不支持的任务由 `create_task` 当场报错；执行后的失败记为 `failed`（§3）。
- **传输**：自带电池跑在独立进程里（§10）。
- **非代码交付物**：用 `put_blob` 按内容哈希分块上传，小报告内联，不接受 worker 本地路径（§6）。
- **能力扩展**：核心能力跟着协议版本走；扩展用 `x-<作者>.<名字>`，不认识的忽略（§8）。
- **`mismatch_flagged` 是核心事件**，不放进扩展（§5）。
- **turn 异常结束时 attempt 停在 `blocked`（`need` 为 `resume`），用回复这次停下的 `send_message` 续接**；`failed` 只用于 attempt 级的失败（§3）。turn 正常结束却没有汇报、也没有排队的消息时同样处理（§10）。

**待决**
- **`progress` 的粒度和频率**：暂时没有想法，做到时再考察（用户定）。
