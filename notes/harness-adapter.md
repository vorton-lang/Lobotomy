# Harness Adapter、执行现场与成果

> Status: 设计决定 v0.3（2026-10-03）
> v0.1 的 git worktree + rebase + 快进同步方案已由 [#5](https://github.com/vorton-lang/Lobotomy/issues/5) 的已确认评论与 [#6](https://github.com/vorton-lang/Lobotomy/issues/6) 的推荐实现取代：SQLite 是业务事实的唯一权威，jj 承载不可变成果，工作目录只是执行现场。
> v0.3 按 [#7 的讨论补充](https://github.com/vorton-lang/Lobotomy/issues/7#issuecomment-5953401457)修订：中断后的范围（1.7）、turn 身份（1.3 第 8 条）、`done` 与采集的顺序（4.1）、验收与发布的原子边界（4.2）、后端与 harness 的生命周期绑定（1.8）。其中 1.8 仍是 #7 的建议，其余已在 [data-model.md](data-model.md) 的讨论中确认。
> 实测环境：Windows 11，Claude Code 2.1.283，Codex CLI 0.159.2（随 Codex desktop app 分发）。

## 0. 原则

- **机械操作归运行时，判断归 Manager。** Manager 是 agent，会犯错；能用确定性规则完成的事不经过它，它只收到事件用于知情。
- **只按正常方式调用官方 CLI。** 不依赖 Claude Agent SDK、Codex SDK / app-server 或 ACP adapter。订阅绑定官方入口，正常调用 CLI 与用户手动使用一致。CLI 做不到的，由 Lobotomy 的 MCP 服务提供。
- **不依赖平台特有行为。** 能力取 Windows 与 Linux 的交集。先在 Windows 上开发运行，正式发版前集中修 Linux 问题。下文标为"Windows 实测"的细节属于平台适配层，不是设计依赖。
- **不依赖 harness 的权限系统。** 所有 role 关闭审批。安全靠结构：成果采集可恢复、后端与开发副本分离、对外能力裁剪。
- **harness 自己的日志和会话文件不管理。**

## 1. Adapter

### 1.1 进程模型：每轮一个进程

每个 turn 启动一次 CLI，结束即退出；跨 turn 的连续性完全靠 harness 原生的 resume。

- 与用户手动执行命令相同；
- 后端重启后，下一轮照常 resume。上一轮的结果不明时，按 1.3 第 6 条处理；运行时不自动恢复被中断的 turn（见 1.7）；
- 两家 CLI 形状一致，不需要监管长驻子进程；
- prompt cache 在服务端按前缀命中，与进程是否常驻无关（实测命中）。

代价是每轮约 2 秒启动开销，以及 turn 进行中不能插话。发给 role 的消息先进入它的收件箱，在当前 turn 结束后投递。用户可以组合现有操作让 agent 提前看到消息：先发消息，再中断当前 turn，再选择继续。运行时不阻止这种组合，也暂不为它提供专门命令（见 [ideas.md](ideas.md)"中断并发送"）。

### 1.2 调用形式

```text
Claude（消息经 stdin）
  首轮：claude -p --session-id <uuid> --output-format stream-json --verbose
          --include-partial-messages
          --dangerously-skip-permissions --model <m>
          --append-system-prompt(-file) <role>
          --strict-mcp-config --mcp-config <cfg>
          --disallowed-tools <见 1.6>
  续用：claude -p --resume <uuid> ...（参数同上，每轮都带）
  fork：claude -p --resume <uuid> --fork-session ...

Codex（消息经 stdin，位置参数为 -）
  首轮：codex exec --json --dangerously-bypass-approvals-and-sandbox --ignore-rules
          --ignore-user-config --disable apps --disable computer_use --disable browser_use
          -m <m> -c developer_instructions=... -c mcp_servers.lobotomy.url=... -
          （session id 取自 thread.started 事件）
  续用：codex exec resume <id> --json ...（参数同上）
  fork：codex exec fork <id> ...
```

### 1.3 Adapter 规则

1. **每轮都在 role 的执行现场启动进程。** 两家 resume 都不恢复 cwd，turn 在进程 cwd 中执行。
2. **每轮都传角色指令与 MCP 配置。** MCP 配置与 1.6 的能力裁剪参数都是启动参数，不保存在 session 中；每个 turn 是新进程，所以每个 turn 都要传。Claude 在压缩后按当轮进程参数重新生成并固定 system prompt；压缩后首轮若未传 `--append-system-prompt`，角色指令会从 system prompt 中永久消失，之后再传也被忽略，直到下一次压缩。模型仍能从压缩摘要中说出角色名，所以问题不显眼。Codex 不需要重传，传了也无害。system prompt 固定后重复传参不影响缓存。
3. **消息走 stdin，长配置走文件。** 命令行长度有平台上限（Windows 约 32K 字符）。Codex 在 stdin 非 TTY 时会读取 stdin 并追加为 `<stdin>` 块，必须显式提供或关闭 stdin。Claude 的 `--mcp-config` 接收多个值，会把其后的位置参数（提示词）当作配置文件路径吞掉（实测踩到）。
4. **压缩：** Claude 从 stdout 的 `system/compact_boundary` 读取（含 trigger、压缩前后 token 数）。Codex 的压缩设计上不影响工作，不做追踪；可尽力读 rollout 文件中的 `compacted` 记录，仅用于 GUI 显示。
5. **用量：** Claude 每轮 `result.usage`，另有 `rate_limit_event`（5 小时 / 7 天额度利用率与重置时间），用于 GUI 显示和识别额度被拒；v1 不按利用率提前限流。Codex 的 `turn.completed.usage` 实测为线程累计值，需与上一轮做差；额度信息不在 `--json` 输出中，只在 rollout 文件的 `rate_limits` 中。额度域、被拒与恢复见 [data-model.md](data-model.md) §8。
6. **停止与中断（#6 §3）：**
   - 收到正常停止请求后，运行时等待当前 turn 结束和 CLI 退出。运行时不启动下一个 turn。
   - 收到立即中断请求后，运行时调用 harness 的原生中断路径，并等待 CLI 退出。
   - 子进程由 harness 自行清理；Lobotomy 不自建进程树管理。Codex 在 Windows 上已实测成立，包括被直接终止的情况（§1.4）。Claude 与 Linux 上仍是设计假设。
   - Windows 上的原生中断是 Ctrl+C，发给目标进程独占的控制台（§1.4、§1.8）。
   - CLI 未退出或执行状态不明时，该 turn 保持"待对账"。运行时保留占用，不重放，也不为同一 native session 或执行现场启动替代执行（[#2](https://github.com/vorton-lang/Lobotomy/issues/2)）。
   - 运行时不恢复被中断的执行现场。保留哪些内容、提供哪些入口，见 1.7。"待对账"只核对 CLI 是否已经退出，见 [data-model.md](data-model.md) §3.3。
7. **事件解析要容错。** 两家 JSON 事件格式都不在稳定承诺内：未知事件忽略，并记录原文。
8. **启动前登记 turn。** 运行时在启动 CLI 前生成稳定的 turn_id，并绑定 task、attempt、native session、执行现场 generation 和本轮投递的输入消息 ID。Claude 首轮的 session ID 由运行时经 `--session-id` 指定；Codex 首轮的 session ID 在 `thread.started` 事件返回后补记。同一 attempt 内接续时，运行时只新建 turn，不新建 attempt（[data-model.md](data-model.md) §3.1、§4.1）。
   - 如果 native session 已有正常结束的 turn，却没有记下 session ID，运行时不再为它登记 turn，role 停下。否则下一个 turn 会不带 resume 启动，悄悄丢掉上下文。用户可以新建 native session（[#10](https://github.com/vorton-lang/Lobotomy/issues/10)，用户确认，2026-10-04）。

### 1.4 实测结果（Windows）

| 项目 | Claude Code | Codex |
|---|---|---|
| 每轮单进程 | 首个输出约 2s，简单一轮 4–5s（haiku） | 首轮首个输出约 2.7s，resume 约 0.2s，一轮 7–22s |
| 跨进程 prompt cache | 命中约 23.6k token，1h TTL | 输入大部分来自缓存 |
| HTTP MCP 注入 | `--mcp-config` 可用 | `-c mcp_servers.<name>.url=` 可用 |
| MCP 协议版本（2026-10-04） | 2026-07-28：先发 `server/discover`，没有 `initialize` 握手，每个请求在 `_meta` 中带协议版本 | 2025-06-18：`initialize` 握手后 `tools/list`、`tools/call` |
| 角色指令 | `--append-system-prompt` 可用，压缩后须重传 | `-c developer_instructions=` 可用，resume、压缩后保留 |
| resume 时的 cwd | 不恢复 | 不恢复 |
| 压缩事件 | stdout 有；`-p "/compact"` 可手动触发 | 压缩会发生，但 `--json` 输出无事件，仅 rollout 文件有 |
| fork | 可用，返回新 id | 可用，返回新 id |
| 模型文本流式 | 加 `--include-partial-messages` 后逐块到达（`stream_event` / `content_block_delta`） | 不流式：`agent_message` 完成后整条到达 |
| 工具输出流式 | 不流式：命令结束后一次给出完整 `tool_result`；之前只有 `tool_use` 和不带输出的 `system/task_started` | 不流式：`item.started`（带命令，in_progress）之后，`item.completed` 一次给出完整 `aggregated_output` |
| turn 结束事件到进程退出 | 约 0.5s | 约 4s |

| 每个 turn 更换 MCP URL | 未测（M3） | 不影响缓存：第二个 turn 的缓存命中均为 98.7%，与不更换相同 |
| 全局指令文件 | 未测（M3） | `--ignore-user-config` 或 `-c project_doc_max_bytes=0` 下，`~/.codex/AGENTS.md` 仍然生效 |

流式与退出间隔三行来自一次探针（[spikes/harness-cli/stream-probe.mjs](../spikes/harness-cli/stream-probe.mjs)，2026-10-04）：命令每秒打印一行，共 5 秒。MCP URL 与全局指令文件两行分别来自 [cache-probe.mjs](../spikes/harness-cli/cache-probe.mjs) 与 [agents-md-probe.mjs](../spikes/harness-cli/agents-md-probe.mjs)（2026-10-04）。两家的事件格式不在稳定承诺内，CLI 升级后需重测。

**Codex 中断实测**（Windows 11，Codex CLI 0.159.2，2026-10-04，[spikes/win-proc](../spikes/win-proc)）。turn 运行一条每秒写一行、共 40 秒的命令，在约 18 秒时用以下方式结束：

| 结束方式 | codex.exe | 工具命令 | rollout | resume |
|---|---|---|---|---|
| 后端崩溃：Job 内的父进程被终止 | 被 Job 终止 | 随之结束 | 只有工具调用，没有工具结果 | 正常。Codex 补一个 "aborted" 工具结果，stderr 打印一行 ERROR |
| 直接终止 codex.exe，不用 Job | 终止 | 随之结束 | 同上 | 同上 |
| Ctrl+Break 发给独立进程组 | 立即退出（0xC000013A） | 随之结束 | 同上 | 同上 |
| Ctrl+C 发给目标进程独占的控制台 | 约 2–5 秒后退出 | 随之结束 | 记录工具结果（"aborted by user after …"）和 `turn_aborted` | 正常，没有 ERROR |

- 四种方式下，工具命令都没有残留进程，包括不用 Job 直接终止 codex.exe 的情况。
- 只有 Ctrl+C 是优雅中断。Ctrl+C 发给整个控制台：发给共享控制台时，同一控制台上的所有进程都会收到。定向中断要求每个 harness 进程有自己的控制台（`CREATE_NO_WINDOW`），由辅助进程临时 attach 到目标控制台后发送。实测中，另一控制台上的旁观进程不受影响。
- 所有方式下，`--json` 输出都停在 `item.started`，没有被中断 turn 的结束事件。运行时按 interrupted 记录（[data-model.md](data-model.md) §3.2）。
- resume 后，模型能说出最后执行的命令，并知道它被中断、没有收到输出。

### 1.5 二进制定位

adapter 按平台自动探测两家 CLI，并允许手动配置；每次启动记录版本。Windows 实测：Claude 经 scoop shim 位于 PATH；Codex 不在 PATH 上，位于 `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe`，hash 目录随 desktop app 更新变化，版本可能在运行期间升级。

### 1.6 能力裁剪

role 会话默认继承用户的全部对外通道：Claude 继承 claude.ai 连接器（Docs、Slack、Drive），Codex 继承 ChatGPT 账号的 apps 连接器（GitHub 写操作、Outlook 发信、Drive 分享、Sites 部署等），以及 computer-use 和浏览器控制。这些对写代码没有用，却是以用户身份对外发送内容的主要渠道。做法不是加审批，而是在启动参数中去掉能力，零延迟、零 token。对所有 role 生效，Manager 也包括在内。

| 能力 | Claude | Codex |
|---|---|---|
| 用户的连接器 | `--strict-mcp-config`（实测只剩 Lobotomy MCP） | `--ignore-user-config --disable apps --disable computer_use --disable browser_use`（实测只剩执行、改文件、网页搜索、生图、子 agent） |
| 绕开 Lobotomy 的内置工具 | `--disallowed-tools CronCreate CronDelete CronList ScheduleWakeup RemoteTrigger PushNotification SendMessage ListAgents EnterWorktree ExitWorktree DesignSync` | — |
| git push | 进程环境变量 `GIT_CONFIG_*` 注入 `url.lobotomy-push-disabled://.pushInsteadOf`（`https://`、`git@`、`ssh://`）；实测 push 在本地失败，fetch 不受影响 | 同左 |
| gh | `GH_CONFIG_DIR` 指向空目录，并从进程环境中删除 `GH_TOKEN`、`GITHUB_TOKEN`、`GH_ENTERPRISE_TOKEN`、`GITHUB_ENTERPRISE_TOKEN`（这四个变量优先于配置目录中的凭据，[#10](https://github.com/vorton-lang/Lobotomy/issues/10)），等同未登录。额度检查调用同样处理 | 同左 |

- 保留网页搜索与读取、子 agent。
- 这些措施防的是失误和过度热心，不防对抗：bypass 模式下 agent 总能绕过（例如从凭据管理器取 token）。
- 运行时每轮从 Claude 的 `system/init` 事件读取工具清单，与已知清单比对；CLI 升级后出现的新工具在 Inspector 中标出，由用户决定是否禁用。
- Codex role 不读用户的 `config.toml`，模型、推理强度等由 Lobotomy 显式传入。
- 外部系统（GitHub 等）由 Lobotomy 统一接入，role 经 Lobotomy MCP 访问，见 [manager-actions.md](manager-actions.md)。

### 1.7 中断后的范围（#7）

Lobotomy 不恢复被中断的执行现场。

- 运行时不重建工作目录，不还原工具的执行位置，也不自动恢复或重放被中断的 turn。
- 运行时保留三类内容：已知的业务事实、已固定的成果、结果不确定的 turn 记录。
- 运行时提供两个入口：经 harness 原生 resume 启动新 turn，或新建 native session。
- 流式输出中尚未完成的部分（partial）只保存在后端内存中。后端崩溃时，partial 丢失（[frontend.md](frontend.md) §3 第 2 条）。

用户主动"继续"时，运行时这样调用（[data-model.md](data-model.md) §9.2）：

1. 运行时经原生 resume 启动新 turn。
2. 本轮输入告知 harness：上一个 turn 已中断，请先检查当前 cwd。

这种调用方式不承诺回到中断的位置。

本节的"不重建工作目录"针对被中断的 turn：目录保持中断时的样子，运行时不还原。§3 的重新物化是另一件事：运行时在采集完成后为下一份工作准备槽位，或在自己的 checkout 中断、就绪状态不明时重写槽位（#6 §2）。两者不冲突。

### 1.8 后端与 harness 的生命周期绑定（#7 建议）

后端退出时，操作系统默认不保证后端启动的 harness 进程一起退出。#7 建议加一层很薄的平台启动适配，把后端与它直接启动的 harness 进程绑定。harness 仍负责自己的子进程；Lobotomy 不扩展成自建的进程树监管器。

| 平台 | 做法 | 效果 | 实现时注意 |
|---|---|---|---|
| Linux | 子进程 exec 前设置 [`PR_SET_PDEATHSIG`](https://www.man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)`=SIGTERM` | 请求 harness 退出并尽量清理，不保证强制终止 | 处理设置之前父进程已退出的竞态。信号针对创建子进程的父线程，需注意启动线程的生命周期 |
| Windows | 带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 [Job Object](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)，句柄由后端独占且不可继承 | 最后一个句柄关闭时，终止已关联的进程；不是优雅退出 | 处理好进程创建与加入 Job 的顺序 |

后端正常关闭时：

1. 运行时调用 harness 的原生中断或退出路径。
2. 运行时等待 CLI 退出。

OS 绑定只在后端异常退出时兜底。进程结束不等于业务成功。

两个平台的效果不同：Linux 只请求退出，Windows 直接终止。按 §0 的交集原则，后端重启后仍按 1.3 第 6 条核对 CLI 是否已退出，不假定 harness 已经结束。

**Windows 实现要点**（2026-10-04 实测，[spikes/win-proc](../spikes/win-proc)）：

- Job Object 绑定有效：父进程被终止后，Job 内的 codex.exe 随之终止。
- 先启动再加入 Job，中间有一个短暂窗口，子进程可能在加入前启动孙进程。实现时以挂起方式创建进程，或使用 `PROC_THREAD_ATTRIBUTE_JOB_LIST`。
- Rust 标准库的 `Command::spawn` 在 Windows 上会让子进程继承父进程所有可继承的句柄。实测中，一个子进程因此持有父进程的 stdout 管道，使调用方一直等到它退出。后端启动 harness 时，要用 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 限定可继承的句柄，避免后端的管道与 socket 泄漏给长期运行的 harness。
- 原生中断：每个 harness 进程以 `CREATE_NO_WINDOW` 启动，拥有自己的控制台。中断时，由一个短命的辅助进程 attach 到目标控制台并发送 Ctrl+C。控制台是进程级状态，不在后端进程内 attach。

**Windows 实现**（2026-10-04，[backend/crates/harness/src/process.rs](../backend/crates/harness/src/process.rs)）：

- 每个 turn 一个 Job。进程以挂起方式创建（`CREATE_SUSPENDED | CREATE_NO_WINDOW`）。运行时把它加入 Job，在 SQLite 中记下 pid 与进程启动时间，然后才让它运行。
- **语义更新：** CLI 退出后，运行时关闭该 turn 的 Job，CLI 留下的进程随之结束，然后才采集。这样采集时没有进程还在写槽位（§4.1 "确认写者已停止"）。代价是 agent 在 turn 中启动的后台进程（例如开发服务器）不会存活到 turn 之后。
- 句柄继承：没有使用 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`，因为 Rust 稳定版的 `Command` 不能设置这个属性。改为后端启动时把自己的 stdin、stdout、stderr 设为不可继承。标准库打开的文件和 socket 本来就不可继承；标准库为子进程准备管道时持有锁，管道不会泄漏给同时启动的其他子进程。
- Ctrl+C 辅助进程是后端程序自身：`lobotomyd ctrl-c <pid>`。
- 已知缺口：后端如果恰好在创建进程与加入 Job 之间崩溃，会留下一个挂起、从未运行的进程。这个窗口极短，暂不处理。
- 测试：[backend/crates/harness/tests/process.rs](../backend/crates/harness/tests/process.rs)（挂起、存活检查、关闭 Job 结束孙进程），[backend/crates/lobotomyd/tests/backend.rs](../backend/crates/lobotomyd/tests/backend.rs)（经辅助进程中断）。

## 2. MCP 服务

后端在 localhost 提供 streamable HTTP MCP。运行时每轮把带 token 的 URL 传给 CLI（1.3 第 2 条），由 URL 识别调用者；每轮不需要额外启动子进程。工具集见 [manager-actions.md](manager-actions.md) 与 [roles-and-tasks.md](roles-and-tasks.md)。

**实现**（2026-10-04）：

- 使用官方 Rust SDK rmcp 3.5（[m1-plan.md](m1-plan.md) §2）。URL 为 `/mcp/{token}`；路由先取出 token，放进 HTTP 请求的扩展中，工具处理函数从请求上下文读取。
- 服务不保存 MCP 会话（rmcp 的 stateless 模式），工具调用以单个 JSON 响应返回，不用 SSE 流，不提供 GET 流（返回 405）。调用者已由 URL 中的 token 识别，会话没有额外作用。
- `Host` 只接受 loopback 名称，防止 DNS 重绑定。
- 两家 CLI 协商的协议版本不同（§1.4）：Claude 已使用 2026-07-28 的无状态协议，Codex 仍使用 2025-06-18 的 `initialize` 握手。stateless 模式对两者逐个请求应答，两者都能工作。

**契约测试**：[backend/crates/lobotomyd/tests/mcp_contract.rs](../backend/crates/lobotomyd/tests/mcp_contract.rs)。用真实 CLI 跑一个 turn，确认三点：CLI 调用了工具，处理函数从 URL 取到了 token；所有 HTTP 交换都成功；工具结果到达了模型。测试打印协商的协议版本和每个 HTTP 交换。它消耗订阅额度，默认不运行。CLI 或 rmcp 升级后重跑：

```text
cargo test -p lobotomyd --test mcp_contract -- --ignored --nocapture --test-threads 1
```

2026-10-04 的结果：Claude Code 2.1.283 与 Codex CLI 0.159.2 都通过。

**语义更新：** v0.2 写的是"每个 role 分配一个 URL"，只能识别到 role。现改为按 turn 发放 token：`done` 等 MCP 调用绑定到发出它的 turn，不按"角色当前任务"反查归属（[data-model.md](data-model.md) §3）。

## 3. 执行现场

| Role | 执行现场 |
|---|---|
| Angela（manager） | 用户的主仓库，即集成版本的只读预览（见 4.3） |
| Binah（tech_lead） | 槽位 `tl` |
| Malkuth（worker） | 槽位 `worker` |
| Yesod（reviewer） | 在固定的候选成果上单独物化的审查现场；现场中的改动不影响成果 |

Manager 与用户看到的是同一份代码。角色定义见 [roles-and-tasks.md](roles-and-tasks.md)。

- **执行现场不是事实来源。** 工作目录只是 CLI 的执行环境，不能反过来推断业务事实；业务事实以 SQLite 为准（#6 §1）。
- **只有 Lobotomy 创建执行现场。** 不使用 `claude -w`、`codex --worktree`，`EnterWorktree` 已在 1.6 中禁用。
- **位置在仓库外**，位于 Lobotomy 的数据目录中，避免被 ripgrep、IDE 索引和测试运行器扫描。
- **槽位固定、复用**，磁盘占用有结构性上限。被忽略的依赖与构建缓存在重新物化时保留（下文第 2 种情况除外）。每个目录都有归属记录与回收条件（[data-model.md](data-model.md) §6）。
- **物化**：每个槽位是一份独立的 git clone，对象经 alternates 与私有存储共享，HEAD 设为本轮基线，使 agent 的 `git status` / `git diff` / `git log` 正好反映本轮工作。不用 linked worktree：它与主库共享 ref，agent 的 `git branch -D`、`git gc` 等操作可能删掉成果的保留 ref。agent 在 clone 中的提交只是草稿，不是同步单位。

  实现（2026-10-04）：
  - jj 的工作副本状态存放在 Lobotomy 的数据目录中，不在槽位里。
  - 槽位的分支名与用户仓库的分支相同。
  - 每次物化时，运行时先对目录做一次快照，快照不跟踪新文件；然后检出目标。结果是：被改动或缺失的已跟踪文件恢复为目标内容；没有被采集过的新文件被删除；被忽略的文件保留。
  - **不转换换行符**（用户确认，2026-10-04）：jj 按原样保存与写出文件内容。槽位中的 git 设 `core.autocrlf=false`，覆盖用户与系统的配置。理由：Windows 上 Git 的系统配置常开着 `autocrlf=true`。实测（Git 2.49，Windows）在这种配置下，CRLF 的改动不出现在 `git diff` 中；`git checkout -- <文件>` 写回的是 CRLF，jj 会把它采集为每一行都改了。
- **重新物化**只在三种情况下发生：
  1. 槽位要交给另一份工作时。前提是上一份工作的现场已经采集完成。
  2. 运行时的检出无法让目录等于目标时：目标中的某个文件被一个未跟踪的文件挡住，通常是被忽略的文件。运行时把旧目录移出槽位路径（隔离），在原路径以新 generation 重新物化，新 generation 的构建缓存是冷的。新 generation 就绪后，运行时删除隔离的目录。隔离只是中间状态，不是处理完毕。

     **语义更新**（2026-10-04）：原来检出中断、或就绪状态不明时（#6 §2）也隔离。实现中每次物化前先快照，重新物化会补完中断的检出：快照把写了一半的文件识别为改动，把检出新加的文件识别为未采集的新文件，检出再把它们改回目标。这两种情况因此改为原地重做（data-model.md §5）。
  3. 任务回到执行、候选成果在验证时已被 rebase 到新的集成版本时。槽位重新物化为 rebase 后的候选成果（[data-model.md](data-model.md) §4.3）。

  第 1、3 种情况在原路径进行。
- **验证现场**：检查在固定的验证现场运行，不在执行者的槽位中运行。每次验证时，运行时把验证现场物化为 rebase 后的候选成果，保留被忽略的构建缓存（data-model.md §5）。
- **采集不触发重新物化。** "采集完成"只是复用槽位的前提。被中断 turn 的现场被采集后，目录保持原样，直到用户做出选择（用户确认，2026-10-03）：

  | 用户选择 | 槽位目录 |
  |---|---|
  | 继续（resume） | 保持原样（见 1.7） |
  | 新建 native session | 保持原样；仍是同一个 attempt |
  | 放弃任务 | 交给下一份工作前重新物化；半成品保留在采集记录中 |
- **审查现场**：Reviewer 不借用 Worker 的槽位，而是在固定的候选成果上单独物化一份现场。成果不可变，Reviewer 在现场中的任何改动都不会进入成果，因此不需要审查前后比对与恢复，也消除了与 Worker 槽位的占用冲突（[#1](https://github.com/vorton-lang/Lobotomy/issues/1)）。现场随任务关闭回收。

## 4. 成果与集成

### 4.1 采集与发布（#6 §2）

```text
① SQL intent    持久保存操作身份、task / attempt、条件版本、预期 revision、现场 generation、采集范围
② jj capture    确认写者已停止后，按声明范围采集文件树，固定 commit_id 并建立 pin
③ SQL publish   短事务发布成果引用，校验 revision 与 expected-current，连同 outbox 一并提交
```

- 已有 pin 时，恢复与重试始终使用同一份成果；没有 pin 时保持 pending，确认写者停止后再决定是否采集。
- **`done` 可能早于 CLI 退出。** 收到 `done` 后，运行时先持久保存采集意图，并保持现有占用。CLI 退出后，运行时再采集并固定成果。turn 结束、任务完成、参与者释放是三个不同的状态变化，不能互相代替。
- 迟到或过期的成果保留来源，但不能推进当前状态。
- **采集时机**：每个 turn 结束、CLI 退出后，运行时采集一次。报告 `done` 的 turn，其采集就是候选成果；被中断的 turn，其采集就是中断现场（[data-model.md](data-model.md) §4.1）。
- **成果是工作区快照**：采集只看采集时工作区中的文件，以槽位的基线为父提交生成一个提交。agent 在槽位中的提交、分支切换、HEAD 移动都不影响成果，成果的 diff 始终是工作区对比基线。
- **采集范围由运行时声明**，不只依赖 harness 可修改的 ignore 文件（[#3](https://github.com/vorton-lang/Lobotomy/issues/3)）：
  - 运行时读取采集时工作区中的 ignore 规则，agent 新增的规则立即生效。
  - 基线中已跟踪的文件始终采集，即使后来被 ignore 规则匹配。预期这是 jj 的默认行为，实现时确认。
  - 项目配置可以声明强制采集的路径（`force_tracking_matcher`），以及始终排除的路径（例如 `target/`、`node_modules/`）。排除的路径在重新物化时按缓存保留。
  - `.gitignore` 的修改本身属于成果，验收时在 diff 中可见。
- **体积护栏**：一次采集中新增文件的总大小或数量超过阈值时，运行时在 pin 之前停止，不写入 jj。
  - `done` 的采集：运行时把文件清单作为模板消息发回执行者。执行者补 ignore 规则或删除文件后，重新报告 `done`。用户也可以确认放行。
  - 其他 turn 的采集：记为"本次跳过"并在 GUI 中显示，下一次采集再试。
  - 阈值在 M1 中用真实项目测量典型新增量后确定。
- 特殊文件、嵌套仓库等 fail closed：报告未覆盖的内容并保留现场（#3）。
- **保留现场不是处理完毕。** 任务在 GUI 中显示为阻塞，并列出未覆盖的内容。以下两件事之一发生后，槽位才能复用：
  1. 原因消除后，运行时重新采集成功；
  2. 用户明确确认丢弃未覆盖的内容。
- 每轮采集取代了 v0.1 的 `git stash create` 快照。

**实现**（2026-10-04，第 3 个增量）：

1. turn 结束时，运行时在结束 turn 的同一个事务中写入采集意图。报告 `done` 的 turn，其采集标为候选成果。
2. CLI 退出后，运行时先遍历槽位。遍历的规则与 jj 的快照相同：各级 `.gitignore`、项目配置的排除路径与强制采集路径。遍历找出两类内容：
   - 新增的文件，交给体积护栏判断；
   - 采集无法保存的内容：嵌套仓库（含 `.git` 或 `.jj` 的子目录）、特殊文件、不是 UTF-8 的文件名。jj 的快照会静默跳过这些内容，嵌套仓库中已跟踪的文件还会被记成删除。因此运行时在快照之前 fail closed。
3. 两项检查都通过后，jj 快照目录。运行时以槽位的基线为父提交写入提交，并以采集记录的 ID 建立 pin（私有存储中的 ref `refs/lobotomy/pins/<id>`）。这个 pin 已存在时，运行时直接使用已固定的提交，不再看目录。
4. 运行时在一个短事务中发布结果。候选成果 pin 成功时，attempt 结束，任务进入验证。

- 已跟踪的文件被 ignore 规则匹配后仍被采集：已由测试确认（上文"预期这是 jj 的默认行为"）。
- **语义补充**：候选成果的采集被挡下时，无论是超过体积护栏，还是有无法保存的内容，运行时都把清单作为模板消息发回执行者，attempt 继续。原文只为体积护栏写了发回执行者。无法保存的内容同样能由执行者处理（删除嵌套仓库中的 `.git`，或把它加入 `.gitignore`），因此按同一方式处理。
- 用户的"放行新增文件"与"丢弃未覆盖内容"只作用于 role 最近一次采集，并且要求 role 没有未结束的 turn，这样槽位仍是被采集时的样子。运行时按用户的选择重新采集。执行者的下一个 turn 开始后，被挡下的候选成果就不能再放行。用户想保留这些文件时，可以调高项目配置中的阈值，再让执行者重新报告 `done`。
- 体积护栏的起始值为 1000 个文件、50 MB。这是占位值，仍按上文用真实项目测量后确定。
- **"撤销这一轮"推迟到 M1 之后。** 它把槽位改写为上一个 turn 的采集，属于第 4 种重新物化（§3）。加入时作为用户命令，前提是 role 没有未结束的 turn，且当前现场已经采集。

### 4.2 集成：只发布已验收的成果

集成版本只沿一个方向前进，只接收已验收的成果。回滚不会出现在控制面中。

- **验证阶段**：运行时用 jj 把候选成果 rebase 到当前集成版本上。jj 把冲突作为数据记录在提交中，rebase 不会中断；有冲突时以运行时模板消息交还给相应 role 处理。检查命令在 rebase 后的结果上运行。
- **检查命令**由用户在 GUI 中按项目设置，存入项目配置并记录版本。运行时在验证现场按顺序运行，按退出码判断，每条命令有超时。没有设置检查命令时，验证只检查采集完整与没有冲突。
- **证据绑定**：检查、审查与验收的证据都绑定到具体成果、基线与条件版本（[#5](https://github.com/vorton-lang/Lobotomy/issues/5) 已确认的 TASK 方向）。
- **验收即发布**：发布是对集成版本头的 CAS。若集成版本在验证之后已经前进，运行时把候选成果 rebase 到新的头并重新验证，必要时重新审查；不能凭旧证据发布。
- **验收与发布的原子边界：**
  - 候选成果的登记与验收后的发布是两个分开的步骤。
  - 验收请求写明 `candidate_id`、完成条件版本和预期的集成版本头。
  - 运行时在同一个 SQLite 事务中提交四项内容：验收决定、集成版本头的 CAS、任务关闭、预览的 outbox 记录。
  - CAS 失败时，任务不关闭。运行时在新的集成版本上重新验证，旧证据不能直接沿用。
  - 事务已提交而预览物化失败时，只表示预览尚未跟上。运行时不撤销已提交的验收与发布。
- **未验收的候选成果不进入预览，也不能被其他任务依赖。** 需要用到它的任务排在它之后。
- **撤销已发布的代码**只能作为新任务向前发布，例如一个 revert 成果，同样经过验证、审查与验收。

**实现**（2026-10-04，第 3 个增量）：

- rebase 由 jj 的树合并完成：以候选成果的父提交为基，合并当前集成版本与候选成果。冲突记录在提交中，rebase 不中断。写入文件的冲突标记采用 git 的格式。
- 合成的提交以集成版本头为唯一父提交，提交信息与作者按 §4.3，并以验证记录的 ID 建立 pin。验收时发布的就是这个提交，所以验收只是一个 SQLite 事务。
- 检查命令经平台的 shell 运行：Windows 为 `cmd.exe /d /s /c`，Linux 为 `sh -c`。环境与 role 一样经过能力裁剪（§1.6），进程与 role 的 CLI 一样放在 Job Object 中。超时后，运行时终止命令及其启动的进程，记为超时。
- 运行时按顺序运行检查命令，第一条失败后不再运行其余的。stdout 与 stderr 各保留最后 4 MB。发回执行者的消息引用输出的最后 40 行。
- 集成版本头或检查配置变化后，等待验收的任务回到验证（data-model.md §4.5）。

### 4.3 主仓库：集成版本的只读预览

**接入**

1. 用户在 GUI 中选择一个本地 git 仓库。
2. 前提：工作区干净，即已跟踪的文件没有改动，也没有未被忽略的新文件；检出在某个分支上，不是 detached HEAD。不满足时，运行时拒绝接入并列出原因。
3. 运行时在 Lobotomy 的数据目录中为项目建立私有存储，把主仓库当前分支的 HEAD 导入为第一个集成版本，并记下主仓库路径和分支名。

**预览的 git 形态**

- 发布时，运行时把新的集成版本提交取进主仓库，把该分支快进到它，并更新工作区，效果相当于 `git merge --ff-only`。用户在 `git log` 中可以看到这些提交，并可以自行 push。
- 每个验收通过的任务在集成版本中是一个提交。
- 提交信息：任务标题、执行者 `done` 汇报中的摘要，以及 trailer `Lobotomy-Task: <id>`、`Lobotomy-Role: <name>`。
- 作者：用户自己的 git 身份。执行的 role 记在 trailer 中（用户确认，2026-10-04）。
- 主仓库与上次预览不一致时（用户自行提交、切换分支或从上游拉取），运行时停止物化并通知（[data-model.md](data-model.md) §5）。用户把分支和工作区恢复到上次预览的状态后，选择"重试预览"。把上游改动并入集成版本是另一项功能，v1 不做。

**其他**

- 用户的主仓库是集成版本的**单向物化**，由运行时机械写入，不是任何事实的来源。只包含已验收的成果。
- 预览物化经 outbox 执行。物化失败时，预览暂时落后于集成版本（见 4.2）。
- 写入前，运行时确认主仓库的工作区仍等于上次物化的版本。不一致时，运行时停止物化并通知，不覆盖文件（data-model.md §5）。
- 前提是用户不手工修改项目文件（#5）。Angela 在这里只读运行，与用户看到同一份代码。
- 进行中的工作通过 GUI 查看（Inspector 的"改动"页：按执行轮的成果与 diff），不进入预览。
- push 与 GitHub 导出的内容范围、触发和批准单独处理，v1 中 Lobotomy 不 push。

**实现**（2026-10-04，第 3 个增量）：

- GUI 完成之前，接入由第一次启动时的 `lobotomyd --data-dir <目录> --repo <仓库>` 完成。
- 接入时，运行时把用户分支 fetch 进私有存储，并记下用户在该仓库的 git 身份（`user.name`、`user.email`）。没有配置身份时，运行时拒绝接入。
- 物化预览时，运行时把集成版本提交导出为私有存储中的一个 ref；主仓库从私有存储 fetch 这个 ref，再执行 `git merge --ff-only`。主仓库已经在目标版本上时，运行时视为已写入：上一次写入的回执丢失了（data-model.md §5）。
- 集成版本的提交是普通的 git 提交，不带 jj 的 change-id 头。提交信息依次为任务标题、`done` 汇报的标题与正文、两个 trailer。

### 4.4 GUI 可见性

Workboard 按槽位显示当前执行轮、最近一次采集，以及候选成果的状态（验证中 / 冲突交还处理中 / 审查中 / 已发布），点开显示相对基线的 diff。

## 5. 运行时与 Manager 的分工

| 事项 | 负责方 |
|---|---|
| 执行现场物化、重新物化、对账 | 运行时 |
| 采集、pin、发布、集成、预览物化 | 运行时 |
| 冲突与检查失败通知 role | 运行时（模板消息） |
| 压缩、用量、限流与并发控制 | 运行时 |
| push 与 GitHub 导出 | 单独处理（v1 中 Lobotomy 不 push） |
| GitHub 写操作 | 对外操作通道：role 申请 → Manager 判断 → 用户确认 → 运行时执行 |

## 6. 待验证 / 未决

- ~~jj-lib 的版本锁定与封装边界~~：已实现（2026-10-04）。jj-lib 锁定为 0.45.1，封装在 `backend/crates/store` 中，jj 的类型不出这个 crate。只用到 jj 的存储（git backend）、树合并和本地工作副本状态（`TreeState`），不使用 jj 的操作日志与视图：业务事实在 SQLite 中，成果靠 pin 保留。- 原生中断与中断后的 resume：Codex 在 Windows 上已实测（§1.4）。Claude、以及 Linux 上的两家仍待实测。结果也决定 ideas.md 中的"中断并发送"能否加入。
- 全局指令文件：已决定 role 继承用户个人的 `~/.codex/AGENTS.md`，不另开 `CODEX_HOME`（用户确认，2026-10-04）。理由：单独的 home 需要另行登录，M4 之后的接管也只能走 CLI 的 TUI，增加的复杂度不值得。实测 `--ignore-user-config` 不能排除全局 AGENTS.md。Claude 的 `~/.claude/CLAUDE.md` 在 M3 时确认。
- Codex 0.159.2 的 `codex queue`（向已有会话排队一条消息）能否在 `exec` 的 turn 运行中投递消息，待查。若可以，它可能替代 ideas.md 中的"中断并发送"。`codex delete --force <id>` 可以按 ID 删除会话，清理探针或临时会话时使用。
- 输入消息是否进入 harness 的会话记录：turn 在不同时刻中断时，两家 CLI 的会话文件里是否已有本轮输入（data-model.md §3.4）。Claude 额度被拒的情况已有一次记录：输入在报错前写入。
- `-p` stream-json 模式下 Claude 额度被拒的事件形式（data-model.md §8.3）。下次自然发生时记录。Codex 被拒的形式暂不处理。
- 每个 turn 更换 MCP URL 后，Claude 的跨进程 prompt cache 是否仍命中。Codex 已实测不受影响（§1.4）。
- 平台启动适配（1.8）：Windows 已实测；Linux 的设置竞态与启动线程待实测。
- Linux 上重跑实测：[spikes/harness-cli/spike.mjs](../spikes/harness-cli/spike.mjs) 为 Node 脚本，可直接移植。
- 接入方式与条款的调研依据见 [research/harness-interfaces.md](research/harness-interfaces.md)。
- Claude `--append-system-prompt-file`：帮助文本中出现过，尚未实测。
- Codex 的长指令方案：`-p` profile 文件，还是 `model_instructions_file`。暂不需要：Windows 命令行上限约 32K 字符，role 指令预计远小于此，先用 `-c developer_instructions`；指令实际接近上限时再研究。
- Codex `--thread-source` 的取值。
