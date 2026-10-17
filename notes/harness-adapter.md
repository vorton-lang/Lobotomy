# Harness Adapter 与执行现场

> Status: 设计决定，大部分已实现。没有实现的项在正文中标明「未实现」。
> 依据：[#5](https://github.com/vorton-lang/Lobotomy/issues/5) 的已确认评论、[#6](https://github.com/vorton-lang/Lobotomy/issues/6) 的推荐实现、[#7 的讨论补充](https://github.com/vorton-lang/Lobotomy/issues/7#issuecomment-5953401457)。SQLite 是业务事实的唯一权威，工作目录只是执行现场。
> Lobotomy 没有私有存储和槽位，不采集、验证或预览成果，也没有展示分支采用与试用终端（[#86](https://github.com/vorton-lang/Lobotomy/issues/86)，用户定）。执行现场见 §3，成果与验收见 §4。
> 实测环境：Windows 11，Claude Code 2.1.283，Codex CLI 0.159.2（随 Codex desktop app 分发）。

## 0. 原则

- **机械操作归运行时，判断归 Manager。** Manager 是 agent，会犯错；能用确定性规则完成的事不经过它，它只收到事件用于知情。
- **只按正常方式调用官方 CLI。** 不依赖 Claude Agent SDK、Codex SDK / app-server 或 ACP adapter。订阅绑定官方入口，正常调用 CLI 与用户手动使用一致。CLI 做不到的，由 Lobotomy 的 MCP 服务提供。例外：官方 CLI 担任 Angela 表现不佳时，可以改用自建的 API harness（用户定）。
- **角色不绑定 harness。** 每个 role 使用哪个 harness 由配置决定，Angela 也一样。外部测试者没有 Claude，所有 role 都要能在 Codex 上运行（用户定）。
- **不依赖平台特有行为。** 能力取 Windows 与 Linux 的交集。先在 Windows 上开发运行，正式发版前集中修 Linux 问题。下文标为"Windows 实测"的细节属于平台适配层，不是设计依赖。
- **不依赖 harness 的权限系统。** 默认关闭审批与沙箱。管理员不允许时，用户可以把这个 harness 改为自动审批。没有人工审批（1.9）。安全靠结构：后端与开发副本分离、对外能力裁剪。
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
  首轮：claude -p --output-format stream-json --verbose --include-partial-messages
          --session-id <uuid>
          <权限参数，见 1.9> [--model <m>] [--effort <e>]
          --append-system-prompt <角色指令>
          --setting-sources "" --strict-mcp-config --disallowed-tools <见 1.6>
          --mcp-config <本轮的配置文件>
  续用：--session-id 换成 --resume <uuid>，其余参数相同，每轮都带

Codex（消息经 stdin，位置参数为 -）
  首轮：codex exec --json <权限参数，见 1.9>
          --skip-git-repo-check --ignore-rules --ignore-user-config
          --disable apps --disable computer_use --disable browser_use
          [-m <m>] [-c model_reasoning_effort=<e>] [-c skills.config=<用户自己装的 skill，见 1.6>]
          -c developer_instructions=... -c mcp_servers.lobotomy.url=...
          -c mcp_servers.lobotomy.default_tools_approval_mode="approve" -
          （session id 取自 thread.started 事件）
  续用：codex exec resume <id> --json ...（其余参数相同）
```

- 参数由 [harness/src/claude.rs](../backend/crates/harness/src/claude.rs) 与 [codex.rs](../backend/crates/harness/src/codex.rs) 拼出。方括号中的参数可以省略：模型与推理强度来自用户为 role 在这个 harness 上的选择，没有选择时不传，用 CLI 的默认值。测试可以在 CLI 配置（`agent/src/harness.rs` 的 `Cli`）上设一个推理强度，给没有选择的 role 用。
- **模型与推理强度由用户按 role 选择**（用户定）。设置中只有 Angela 一行，harness 旁边是模型和推理强度（frontend.md §7.8）。
  - worker 的 harness 和模型由用户写在 `workers.toml` 中（tech-plan.md §7 第 7 组，#86，用户定）。
  - 每个 role 在每个 harness 上各存一份选择（data-model.md §9.1 的 `role_model`）。依据：模型名属于 harness，role 换到另一个 harness 再换回来时，原来的选择还在。
  - 每个 turn 启动时读取选择。改选择不换会话：正在运行的 turn 不受影响，下一个 turn 用新选择 resume 同一个会话。
  - 可选的模型：Claude 是 CLI 的别名 `fable`、`opus`、`sonnet`、`haiku`。Claude Code 没有列出模型的命令，帮助文本举了前三个。推理强度是 low、medium、high、xhigh、max（Claude Code 2.1.283 帮助文本）。Codex 的模型来自 `codex debug models` 输出的目录：只列 `visibility` 为 `list` 的模型，每个带支持的推理强度和默认强度。
  - 后端在 GUI 第一次要模型列表时读一次 Codex 的目录。这个命令读 Codex 自己的缓存，不调用模型，实测 0.08 秒。它没有 `--ignore-user-config`，会读用户的 `config.toml`。读不到目录时，GUI 让用户直接输入模型名和推理强度。
  - 运行时只检查名字不会被 CLI 当成选项（不以 `-` 开头，只含字母、数字和 `._-:[]/`），不检查模型是否存在。名字不对时，turn 按 CLI 的报错失败。
  - 实测（Windows，2026-10-13）：两家都能在 resume 时换模型，会话的上文还在。Claude Code 2.1.283：haiku 开的会话用 `--model sonnet --effort low` resume，`system/init` 报 `claude-sonnet-5`，模型答出了上一轮的内容；haiku 也接受 `--effort high`。codex-cli 0.162.0-alpha.17.2：`gpt-6-luna` 开的会话用 `-m gpt-6-astra` resume，turn 正常完成，但先输出一条 `type` 为 `error` 的 item："This session was recorded with model `gpt-6-luna` but is resuming with `gpt-6-astra`. Consider switching back to `gpt-6-luna` as it may affect Codex performance."
  - 实测（Windows，2026-10-13，codex-cli 0.162.0-alpha.17.2）：没有选择时，Codex 用 `gpt-6.1-sol`，推理强度是它的默认值 low。同一个 Angela 的任务（派 P2 的第一单，约 5.4 万 token 输入，约 900 token 输出，不带 MCP），`gpt-6.1-sol` 用了 344 秒；`gpt-6-astra` 用默认强度（medium）34 秒，显式指定 medium 30 秒。三份回答的内容相近。每种只测了一次，不代表模型的一般速度。
- Claude 的角色指令按文字内联传入，不用 `--append-system-prompt-file`（§6）。
- 两家都有 fork（`claude -p --resume <uuid> --fork-session`、`codex exec fork <id>`），实测可用，返回新 id（§1.4）。Lobotomy 没有使用 fork。

### 1.3 Adapter 规则

1. **每轮都在 role 的执行现场启动进程。** 两家 resume 都不恢复 cwd，turn 在进程 cwd 中执行。
2. **每轮都传角色指令与 MCP 配置。** MCP 配置与 1.6 的能力裁剪参数都是启动参数，不保存在 session 中；每个 turn 是新进程，所以每个 turn 都要传。Claude 在压缩后按当轮进程参数重新生成并固定 system prompt；压缩后首轮若未传 `--append-system-prompt`，角色指令会从 system prompt 中永久消失，之后再传也被忽略，直到下一次压缩。模型仍能从压缩摘要中说出角色名，所以问题不显眼。Codex 不需要重传，传了也无害。system prompt 固定后重复传参不影响缓存。
   - Claude Code 2.1.283 的 `--system-prompt-snapshot` 默认开启，说明的行为与上面一致：对话的第一次请求时生成并记录 system prompt；之后的请求与 resume 原样发送记录，后来传入的文字不生效，直到对话被压缩（帮助文本，2026-10-09）。所以每个 turn 仍要传角色指令。修改角色指令后，已有的会话要到压缩后、或新建会话后才用上新指令。
3. **消息走 stdin，长配置走文件。** 命令行长度有平台上限（Windows 约 32K 字符）。Codex 在 stdin 非 TTY 时会读取 stdin 并追加为 `<stdin>` 块，必须显式提供或关闭 stdin。Claude 的 `--mcp-config` 接收多个值，会把其后的位置参数（提示词）当作配置文件路径吞掉（实测踩到）。
4. **压缩**（未实现，列入 roadmap 待排）：
   - 设计：Claude 从 stdout 的 `system/compact_boundary` 读取（含 trigger、压缩前后 token 数）。Codex 的压缩设计上不影响工作，不做追踪；可尽力读 rollout 文件中的 `compacted` 记录，仅用于 GUI 显示。
   - 现状：Claude 的适配器收到 `compact_boundary` 后丢弃。运行时不读 Codex 的 rollout 文件。
5. **用量与额度：**
   - 已实现：适配器从 Claude 的 `rate_limit_event` 识别额度被拒（§1.4）。v1 不按利用率提前限流。额度域、被拒与恢复见 [data-model.md](data-model.md) §8。
   - 未实现，列入 roadmap 待排：记录并在 GUI 中显示每轮用量与额度利用率。设计：Claude 取每轮 `result.usage`，以及 `rate_limit_event` 中的 5 小时 / 7 天额度利用率与重置时间。Codex 的 `turn.completed.usage` 实测为线程累计值，需与上一轮做差；Codex 的额度信息不在 `--json` 输出中，只在 rollout 文件的 `rate_limits` 中。
   - 现状：适配器把 turn 结束事件中的 usage 交给运行时，运行时不记录。`status` 不是 `rejected` 的 `rate_limit_event` 被丢弃。
6. **停止与中断（#6 §3）：**
   - 收到正常停止请求后，运行时等待当前 turn 结束和 CLI 退出。运行时不启动下一个 turn。
   - 收到立即中断请求后，运行时调用 harness 的原生中断路径，并等待 CLI 退出。
   - CLI 及其子进程由运行时按 §1.8 结束：CLI 退出后，运行时结束这个 turn 的进程树，并确认其中的进程已退出。Windows 实测中，Codex 被中断或被直接终止时，工具命令都没有残留进程（§1.4）。
   - Claude 在 Windows 上实测（Claude Code 2.1.283，haiku，2026-10-10）：命令运行中发 Ctrl+C，CLI 在 1 秒内退出，并输出一个出错的 `result`。用户要求了中断，运行时把这个 turn 记为 interrupted，不当作失败。继续时 resume 同一个会话，任务照常完成。
   - Windows 上的原生中断是 Ctrl+C，发给目标进程独占的控制台（§1.4、§1.8）。
   - CLI 未退出或执行状态不明时，该 turn 保持"待对账"。运行时保留占用，不重放，也不为同一 native session 或执行现场启动替代执行（[#2](https://github.com/vorton-lang/Lobotomy/issues/2)）。
   - 运行时不恢复被中断的执行现场。保留哪些内容、提供哪些入口，见 1.7。"待对账"只核对 CLI 是否已经退出，见 [data-model.md](data-model.md) §3.3。
7. **事件解析要容错。** 两家 JSON 事件格式都不在稳定承诺内：未知事件忽略，并记录原文。
   - harness 自己的格式只在适配层里解析。MCP 调用的结果由适配层整理成 `result_text`（工具返回的第一段文字）和 `error_text`（调用失败的原因）；运行时和 GUI 只读这两个字段（[#16](https://github.com/vorton-lang/Lobotomy/issues/16)）。
   - 按 harness 不同的代码都对 `Harness` 枚举做匹配。加入新的 harness 时，编译器会指出每一处。
   - **适配器接口**（实现时的选择）：两个真实 harness 都按枚举匹配，没有抽象成 trait。按 harness 不同的只有四处：拼参数、解析输出（`Output`）、识别权限被拒、额度检查。两个都是 CLI，抽出的接口也只会是 CLI 的形状。自建的 API harness（roadmap.md 原则）可以做成同样形状的可执行程序：从 stdin 读输入，按参数接续会话，输出与某一家相同的 JSON 事件，会话记录由它自己保存在 Lobotomy 的数据目录中。这样运行时不需要改动。真有第三种 harness 时，再按它的需要调整。
8. **启动前登记 turn。** 运行时在启动 CLI 前生成稳定的 turn_id，并绑定 task、attempt、native session 和本轮投递的输入消息 ID。Claude 首轮的 session ID 由运行时经 `--session-id` 指定；Codex 首轮的 session ID 在 `thread.started` 事件返回后补记。同一 attempt 内接续时，运行时只新建 turn，不新建 attempt（[data-model.md](data-model.md) §3.1、§4.1）。
   - 如果 native session 已有正常结束的 turn，却没有记下 session ID，运行时不再为它登记 turn，role 停下。否则下一个 turn 会不带 resume 启动，悄悄丢掉上下文。用户可以新建 native session（[#10](https://github.com/vorton-lang/Lobotomy/issues/10)，用户定）。

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
| 每个 turn 更换 MCP URL | 未测 | 不影响缓存：第二个 turn 的缓存命中均为 98.7%，与不更换相同 |
| 全局指令文件 | 未测 | `--ignore-user-config` 或 `-c project_doc_max_bytes=0` 下，`~/.codex/AGENTS.md` 仍然生效 |

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

**Claude 的事件**（Claude Code 2.1.283，haiku，Windows，2026-10-09；样本见 [backend/crates/harness/tests/fixtures/claude-turn.jsonl](../backend/crates/harness/tests/fixtures/claude-turn.jsonl)）。与 Codex 不同，解析要保留状态：

- 新会话的 ID 由运行时生成，经 `--session-id` 传入；`system/init` 中的 `session_id` 与它相同。resume 后仍是同一个 ID。
- 一个 `assistant` 事件只带一个内容块，不带块的序号。顺序是：块开始 → 部分消息 → 这个块的 `assistant` 事件 → 块结束。所以完整的块属于当时打开的块，item 的 ID 是"消息 ID:块序号"，与部分消息的 ID 相同。
- 文本块的部分消息实时显示。工具的输入也会分段到达，但只在完整时显示。
- 工具调用在 `assistant` 事件中开始，结果在之后的 `user` 事件的 `tool_result` 中到达。结果是字符串或内容块数组；失败时 `is_error` 为真，命令的结果以 "Exit code N" 开头。
- 思考块只有签名、没有文字时，不记录。
- 工具按种类归入 item：`Bash`、`PowerShell` 是命令；`Write`、`Edit`、`MultiEdit`、`NotebookEdit` 是文件改动；`mcp__<服务>__<工具>` 是 MCP 调用；`WebSearch` 是搜索；其余（如 `Read`、`Grep`、`TaskCreate`）是通用的工具调用。工作目录中的路径记为相对路径。
- 子 agent 内部的消息（`parent_tool_use_id` 不为空）不进入对话，只显示子 agent 这次工具调用本身。
- `result` 事件结束 turn：`subtype` 为 `success` 且 `is_error` 为假时正常结束，其余为失败。
- `rate_limit_event` 每轮都有，实测 `status` 为 `allowed_warning`。
- 额度被拒：交互式 CLI 中实测过一次（[data-model.md](data-model.md) §8.3）。`-p` 下的形式还没有见到。适配器认三种信号，任一出现即为额度被拒：`status` 为 `rejected` 的 `rate_limit_event`（重置时间取自这里）；顶层 `error` 为 `rate_limit` 的 assistant 消息；`api_error_status` 为 429 的 `result`。被拒时那条合成消息记为错误，不当作 agent 的回复。

**Claude adapter 的实现**：参数与解析在 [harness/src/claude.rs](../backend/crates/harness/src/claude.rs)。

- 运行时按 turn 所在会话的 harness 启动 CLI。role 切换 harness 后，旧会话结束，下一个 turn 在新会话中运行（data-model.md §9.2）。
- 每个 turn 的 MCP 配置写到数据目录的 `turns/<turn_id>.mcp.json`，turn 结束时删除：文件中有这个 turn 的 token。
- 新会话的第一个 turn 用运行时生成的 ID（`--session-id`），之后用 `--resume`。会话 ID 仍以 `system/init` 读回的为准，规则与 Codex 相同（§1.3 第 8 条）。
- 额度检查用 `--no-session-persistence` 和 haiku，被拒时取 `rate_limit_event` 中的重置时间。
- 后端测试用假 Claude CLI（[lobotomyd/tests/fixtures/fake-claude.mjs](../backend/crates/lobotomyd/tests/fixtures/fake-claude.mjs)）覆盖完成到验收、续用会话、额度被拒、中断。

### 1.5 二进制定位

adapter 按平台自动探测两家 CLI，并允许手动配置。`CLAUDE_BIN`、`CODEX_BIN` 指定二进制；`LOBOTOMY_CLAUDE`、`LOBOTOMY_CODEX`（程序与参数的 JSON 数组）整个替换启动命令，GUI 的端到端测试用它们换上假 CLI。

每次启动记录 CLI 的版本：未实现，列入 roadmap 待排。

Windows 实测：Claude 经 scoop shim 位于 PATH；Codex 不在 PATH 上，位于 `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe`，hash 目录随 desktop app 更新变化，版本可能在运行期间升级。

### 1.6 能力裁剪

role 会话默认继承用户的全部对外通道：Claude 继承 claude.ai 连接器（Docs、Slack、Drive），Codex 继承 ChatGPT 账号的 apps 连接器（GitHub 写操作、Outlook 发信、Drive 分享、Sites 部署等），以及 computer-use 和浏览器控制。这些对写代码没有用，却是以用户身份对外发送内容的主要渠道。做法不是加审批，而是在启动参数中去掉能力，零延迟、零 token。对所有 role 生效，Manager 也包括在内。

| 能力 | Claude | Codex |
|---|---|---|
| 用户的连接器 | `--strict-mcp-config`（实测只剩 Lobotomy MCP） | `--ignore-user-config --disable apps --disable computer_use --disable browser_use`（实测只剩执行、改文件、网页搜索、生图、子 agent） |
| 绕开 Lobotomy 的内置工具 | `--disallowed-tools CronCreate,CronDelete,CronList,ScheduleWakeup,RemoteTrigger,PushNotification,SendMessage,ListAgents,EnterWorktree,ExitWorktree,DesignSync,Workflow` | — |
| 用户自己的设置（hooks、权限规则） | `--setting-sources ""`：不读用户、项目和本地的设置文件 | `--ignore-user-config` |
| git push | 进程环境变量 `GIT_CONFIG_*` 注入 `url.lobotomy-push-disabled://.pushInsteadOf`（`https://`、`git@`、`ssh://`）；实测 push 在本地失败，fetch 不受影响 | 同左 |
| gh | `GH_CONFIG_DIR` 指向空目录，并从进程环境中删除 `GH_TOKEN`、`GITHUB_TOKEN`、`GH_ENTERPRISE_TOKEN`、`GITHUB_ENTERPRISE_TOKEN`（这四个变量优先于配置目录中的凭据，[#10](https://github.com/vorton-lang/Lobotomy/issues/10)），等同未登录。额度检查调用同样处理 | 同左 |
| harness 自带的长期记忆 | 进程环境变量 `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`：Claude Code 不读也不写 `~/.claude/projects/<工作目录>/memory/`。`--setting-sources ""` 关不掉它：2026-10-11 Angela 在那里写过文件。依据：长期记忆由 Lobotomy 保存（roadmap.md"原则"） | 未查 |
| 用户自己装的 skill（见下） | 实测不加载 | `-c skills.config=[…]` 按路径逐个关掉 |
| 后台任务（用户定） | 进程环境变量 `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`：命令和子 agent 都在前台运行。依据见下面的实测 | 未查 |

- 保留网页搜索与读取、子 agent。
- 这些措施防的是失误和过度热心，不防对抗：bypass 模式下 agent 总能绕过（例如从凭据管理器取 token）。
- 新工具提示（未实现，列入 roadmap 待排）：运行时每轮从 Claude 的 `system/init` 事件读取工具清单，与已知清单比对；CLI 升级后出现的新工具在 Inspector 中标出，由用户决定是否禁用。现在适配器只从 `system/init` 读取会话 ID 和工作目录；Inspector 也还没有做（[frontend.md](frontend.md) §2）。
- Codex role 不读用户的 `config.toml`。Claude role 同样不读设置文件：用户为自己使用配置的 hooks 和权限规则，假定有人在场。所以用户为自己选的模型和推理强度对 role 都不生效，模型与推理强度由 Lobotomy 按用户在设置中的选择传入（§1.2）。2026-10-13 实测，Lobotomy 不传模型时：用户的 Claude 设置是 `opus`，Angela 的 turn 用的是 `claude-fable-5-1`；用户的 Codex 配置是 `gpt-6-astra` 与 medium，Angela 的 turn 用的是 `gpt-6.1-sol` 与 low。
  - 待决：同一天 Fable 的额度用完，CLI 的提示是可以换用别的模型。额度域现在按 harness 划分（data-model.md §8）；各 role 都用同一个默认模型时，这样划分不会多停 role。现在可以按 role 选模型，额度域是否要细到模型，还没有定。
- **全局指令文件**（用户定）：Codex role 继承用户个人的 `~/.codex/AGENTS.md`，Lobotomy 不另开 `CODEX_HOME`。理由：单独的 home 需要另行登录，以后的接管也只能走 CLI 的 TUI，增加的复杂度不值得。实测 `--ignore-user-config` 与 `-c project_doc_max_bytes=0` 都不能排除全局 AGENTS.md（§1.4）。Claude role 的情况见 §6。
- **用户自己装的 skill 不给 role 用，官方的保留**（用户定）。依据：用户自己的 skill 写的是用户本人的协作与工程习惯，Angela 在 Codex 上的一个 turn 用命令读了 user-collaboration 和 engineering-taste 的 `SKILL.md`。Codex 与插件自带的 skill 关掉会削弱模型的能力。
  - Codex：每个 turn 列出 `$CODEX_HOME/skills`（默认 `~/.codex/skills`）与 `~/.agents/skills` 下的 `SKILL.md`，跳过以 `.` 开头的目录（Codex 自带的 skill 在 `.system`），用 `-c skills.config=[{path=…, enabled=false}, …]` 逐个关掉。路径用正斜杠。仓库自己的 skill 不动。`-c skills.include_instructions=false` 会连官方的一起关掉，不用。
  - Codex 实测（codex-cli 0.162.0-alpha.17.2，Windows，2026-10-13，带 §1.2 的参数）：不关时，模型说能用 imagegen、openai-docs、skill-creator、skill-installer，以及用户的 engineering-taste、great-resume、grilling、grilling-adapter、user-collaboration；按路径关掉后只剩前四个。
  - Claude：用 `--setting-sources ""` 启动时，`system/init` 的 skill 清单中没有用户从 claude.ai 同步的 skill，例如 docx、coding-discipline（2026-10-13）。
- **Claude Code 2.1.283 实测**（Windows，2026-10-09）：
  - 用上面的参数启动后，`system/init` 中的工具清单为 Task、Edit、Glob、Grep、NotebookEdit、PowerShell、Read、ReportFindings、Skill、TaskCreate、TaskGet、TaskList、TaskStop、TaskUpdate、ToolSearch、WebFetch、WebSearch、Write；MCP 服务只有 lobotomy。
  - `Workflow` 是新出现的工具，会在后台启动多个 agent，可能在 turn 结束后仍在运行，所以去掉。子 agent（`Task`）在 turn 内同步完成，保留。
  - Windows 上执行命令的工具是 `PowerShell`，不是 `Bash`。待办列表用 `TaskCreate`、`TaskUpdate` 等工具，没有 `TodoWrite`。
- **后台任务的实测**（Claude Code 2.1.283，haiku，`-p` 加 stream-json，Windows，2026-10-17）：
  - 不设变量时，模型用 `run_in_background` 启动一条 20 秒的命令，回复"已在后台启动"后，turn 以 `result/success` 结束。约 5 秒后，Claude Code 把后台任务标为 `killed`，进程以退出码 0 退出，命令没有跑完。print 模式下，后台任务结束不会再唤醒模型。
  - 设了变量后，工具中没有 `run_in_background` 参数。明确要求放后台时，模型改用 PowerShell 的 `Start-Job`；turn 照样正常结束，作业随进程树结束（§1.8）。所以这个变量只减少这种情况，不能杜绝。worker 宿主对这种情况的处理见 worker-interface.md §10。
  - 设了变量后，同一条回复中派出的两个子 agent 仍然并行运行（`is_backgrounded: false`）：各运行 15 秒，约 18 秒后一起返回，主 agent 等两个都返回才继续。
- 外部系统（GitHub 等）由 Lobotomy 统一接入，role 经 Lobotomy MCP 访问，见 [manager-actions.md](manager-actions.md)。未实现，roadmap 待排（对外操作与导出）。

### 1.7 中断后的范围（#7）

Lobotomy 不恢复被中断的执行现场。

- 运行时不重建工作目录，不还原工具的执行位置，也不自动恢复或重放被中断的 turn。
- 运行时保留两类内容：已知的业务事实、结果不确定的 turn 记录。
- 运行时提供两个入口：经 harness 原生 resume 启动新 turn，或新建 native session。
- 流式输出中尚未完成的部分（partial）只保存在后端内存中。后端崩溃时，partial 丢失（[frontend.md](frontend.md) §3 第 2 条）。

用户主动"继续"时，运行时这样调用（[data-model.md](data-model.md) §9.2）：

1. 运行时经原生 resume 启动新 turn。
2. 本轮输入告知 harness：上一个 turn 已中断，请先检查当前 cwd。

这种调用方式不承诺回到中断的位置。

### 1.8 后端与 harness 的生命周期绑定（#7）

后端退出时，操作系统默认不保证后端启动的 harness 进程一起退出。所以运行时用一层很薄的平台启动适配，把后端与它启动的进程树绑定（[#7](https://github.com/vorton-lang/Lobotomy/issues/7)）。

Lobotomy 负责自己启动的进程树（[#46](https://github.com/vorton-lang/Lobotomy/issues/46)，用户定）。Windows 用 Job Object；Linux 用独立进程组，覆盖 CLI 和仍在这个进程组中的普通子进程。主动脱离进程组或另建 session 的进程不在 Linux 的保证范围内。harness 仍先走自己的中断与清理路径（后端正常关闭时除外，见下文）。依据：Windows 的 Job Object 本来就结束整个进程树，这样两个平台行为一致；归档超时时也要结束 CLI 的进程树（data-model.md §10.4）。

| 平台 | 做法 | 效果 | 实现 |
|---|---|---|---|
| Linux | CLI 在独立进程组中运行；子进程 exec 前设置 [`PR_SET_PDEATHSIG`](https://www.man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)`=SIGTERM` | 正常收尾结束进程组；后端异常退出时请求直接 CLI 退出，不保证强制终止整组 | 设置信号后，子进程检查父进程是否已变：后端在设置之前已经退出时，子进程立即退出。信号跟随创建子进程的线程，所以进程都在后端常驻的异步运行时上启动，不在短命的线程上启动 |
| Windows | 带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 [Job Object](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)，句柄由后端独占且不可继承 | 最后一个句柄关闭时，终止已关联的进程；不是优雅退出 | 进程以挂起方式创建，加入 Job 后才运行（见下文"Windows 实现"） |

实现在 [harness/src/process.rs](../backend/crates/harness/src/process.rs)。

**turn 结束时**：CLI 退出后，运行时结束这个 turn 的进程树，确认其中的进程已退出，然后才结束这个 turn。代价是 agent 在 turn 中启动的后台进程（例如开发服务器）不会存活到 turn 之后。

**后端正常关闭时**（用户定）：运行时不先中断再等待，直接结束 CLI 的进程树。被结束的 turn 在下次启动时照常核对。依据：第 1 步的实际验收中（2026-10-17），Codex 在 10 秒内没有响应 Ctrl+C，用户只看到应用迟迟不退出；强制结束后，任务照样在同一个会话里续接。没有证据表明多等能多留下什么。

未实现，见 tech-plan.md §7"实际验收"。现状是先中断再等待：

1. 运行时调用 harness 的原生中断或退出路径。
2. 运行时等待 CLI 退出。归档超过宽限期时强制结束所属进程组或 Job Object，确认其中的进程已退出，再释放项目。Linux 已退出但尚未回收的僵尸进程不再运行，不阻止这一步。

丢弃 runner、关闭进程句柄和 kill-on-drop 都只作兜底，不能代替正常收尾中的退出确认。

OS 绑定只在后端异常退出时兜底。进程结束不等于业务成功。

两个平台的效果不同：Linux 只请求退出，Windows 直接终止。按 §0 的交集原则，后端重启后仍按 1.3 第 6 条核对 CLI 是否已退出，不假定 harness 已经结束。

**Windows 实测**（2026-10-04，[spikes/win-proc](../spikes/win-proc)）：

- Job Object 绑定有效：父进程被终止后，Job 内的 codex.exe 随之终止。
- 先启动再加入 Job 时，中间有一个短暂窗口：子进程可能在加入之前启动孙进程。
- Rust 标准库的 `Command::spawn` 在 Windows 上会让子进程继承父进程所有可继承的句柄。实测中，一个子进程因此持有父进程的 stdout 管道，使调用方一直等到它退出。

**Windows 实现**：

- 每个 turn 一个 Job。进程以挂起方式创建（`CREATE_SUSPENDED | CREATE_NO_WINDOW`）。运行时把它加入 Job，在 SQLite 中记下 pid 与进程启动时间，然后才让它运行。这样不留上面的窗口。
- 句柄继承：后端启动时把自己的 stdin、stdout、stderr 设为不可继承。标准库打开的文件和 socket 本来就不可继承；标准库为子进程准备管道时持有锁，管道不会泄漏给同时启动的其他子进程。没有用 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 限定可继承的句柄，因为 Rust 稳定版的 `Command` 不能设置这个属性。
- 原生中断：每个 harness 进程以 `CREATE_NO_WINDOW` 启动，拥有自己的控制台。中断时，一个短命的辅助进程 attach 到目标控制台并发送 Ctrl+C。控制台是进程级状态，所以不在后端进程内 attach。辅助进程是后端程序自身：`lobotomyd ctrl-c <pid>`。
- 已知缺口：后端如果恰好在创建进程与加入 Job 之间崩溃，会留下一个挂起、从未运行的进程。这个窗口极短，暂不处理。

**统一的启动函数**（[agent/src/launch.rs](../backend/crates/agent/src/launch.rs)，[#16](https://github.com/vorton-lang/Lobotomy/issues/16)）：后端启动的所有进程都经过它，包括 role 的 turn 和额度检查。

- 每个进程用裁剪过的环境（§1.6），有自己的 Job 或进程组。
- 进程在开始运行时就读取 stderr，然后才写 stdin。这样进程在读输入之前往 stderr 写很多内容，也不会因管道写满而卡住（#10）。

**测试**：

- [backend/crates/harness/tests/process_tree.rs](../backend/crates/harness/tests/process_tree.rs)：在两个平台上，确认结束时 CLI 与它的子进程都已退出；丢弃进程的所有者时也结束整个进程树。
- [backend/crates/worker/tests/runner.rs](../backend/crates/worker/tests/runner.rs)：停止 turn 时经辅助进程中断，不响应中断的 CLI 在宽限期后连同进程树结束。

### 1.9 权限模式

每个 harness 有一项权限设置（#13，用户定）。设置属于本机（data-model.md §10），用于后端运行的 role，也就是 Angela。worker 不读这项设置：它的权限模式写在 `workers.toml` 条目的 `agent.permission` 中，不写时是完全放开（实现时的选择）。Codex 与 Claude 分开设置：同一个环境可能允许一个完全放开，而不允许另一个。依据：#13 的测试环境中，管理员通过 Codex 的 requirements 不允许完全放开，Codex 一启动就退出。

| 模式 | 什么时候用 | Codex | Claude（自动审批未在受管环境中实测） |
|---|---|---|---|
| 完全放开（默认） | 不审批，不用沙箱 | `--dangerously-bypass-approvals-and-sandbox` | `--dangerously-skip-permissions` |
| 自动审批 | 管理员不允许完全放开 | `-c approval_policy="on-request" -c approvals_reviewer="auto_review" -c sandbox_mode="workspace-write"` | `--permission-mode auto --permission-prompts none`：需要提示的操作直接被拒 |
| Lobotomy 的 MCP 工具 | 两种模式都免审批 | `-c mcp_servers.lobotomy.default_tools_approval_mode="approve"` | `--allowed-tools mcp__lobotomy` |

- **没有人工审批。** Lobotomy 不把审批请求交给用户（[research/hci-rationale.md](research/hci-rationale.md)）。`codex exec` 中没有人能回答审批请求，需要审批的操作当场被拒。turn 照常结束，但工作和报告都做不成（实测见下）。
- **不继承用户自己的 Codex 配置。** `--ignore-user-config` 用于能力裁剪（1.6）。用户为交互使用设置的审批方式假定有人在场，不适用于 role。管理员的 requirements 不受 `--ignore-user-config` 影响，照常生效。
- **Lobotomy 自己的 MCP 工具在任何模式下都免审批**：`-c mcp_servers.lobotomy.default_tools_approval_mode="approve"`。否则在完全放开以外的模式下，Codex 可能拒绝 `org_report`（#13 记录的 "MCP tool call requires approval, but approval policy is never"）。
- **自动审批的代价**：沙箱默认断网，不能写工作目录以外的地方。超出沙箱的操作由 Codex 的自动审核决定，多花 token 和时间。
- `--approve-for-me` 等于上表中的三项配置，但它只能放在 `exec` 之后、子命令之前；`codex exec resume` 不接受它（实测报 unexpected argument）。adapter 逐项传配置，首轮与续用的写法相同。
- 每个 turn 启动时读取设置，改动从下一个 turn 生效。额度检查（data-model.md §8.4）用同一设置启动，环境拒绝 turn 时也同样拒绝检查。
- **运行时不自动切换模式。** 被拒时：
  - CLI 在 stdout 上没有任何输出就退出，在 stderr 写出原因。Codex 写的是："`approval_policy = "never"` cannot be used because requirements do not allow `sandbox_mode = "danger-full-access"`; …"。
  - 运行时把这个 turn 记为 failed，失败种类为"权限模式不被允许"，并标明 harness 没有开始这个 turn。
  - "等你决定"卡片说明原因，并提供「改用自动审批并继续」。用户点击后，运行时修改设置，然后继续。继续时原样重发那次的输入（data-model.md §3.4）。
  - 在 ⚙ 设置中可以改回。

**实测**（Codex CLI 0.159.2，Windows，2026-10-04）。每项一个一次性 turn（`--ephemeral`，不留会话），MCP 工具来自一个最小的本地服务：

| 配置 | 操作 | 结果 |
|---|---|---|
| `on-request`，审核者为用户，只读沙箱 | 用命令写文件 | 命令被拒（"blocked by policy"），模型说明会话不允许提权。21 秒后 turn 正常结束，没有等待 |
| `on-request`，审核者为用户，`workspace-write` | 调用 MCP 工具 | 被拒："MCP tool call requires approval, but approval policy is never"。加上 `default_tools_approval_mode="approve"` 后成功 |
| `on-request`，`auto_review`，只读沙箱 | 用命令写文件 | 自动审核放行，文件写成，19 秒结束 |
| `on-request`，`auto_review`，`workspace-write` | 调用 MCP 工具 | 成功，加不加 `approve` 都成功 |
| 完全放开，加 `approve` | 调用 MCP 工具 | 成功 |

受管环境本身没有在本机复现。被拒时的处理用假 CLI 验证，它输出上面那段 Codex 原文。

**实现**：模式定义在 [harness/src/lib.rs](../backend/crates/harness/src/lib.rs)，Codex 的参数在 [codex.rs](../backend/crates/harness/src/codex.rs)，设置存在 `host.db` 的 `harness_setting` 表（[core/src/host.rs](../backend/crates/core/src/host.rs)）。

## 2. MCP 服务

后端在 localhost 提供 streamable HTTP MCP。运行时按 turn 发放 token，每轮把带 token 的 URL 传给 CLI（1.3 第 2 条），由 URL 识别调用者；每轮不需要额外启动子进程。`done` 等 MCP 调用绑定到发出它的 turn，不按"角色当前任务"反查归属（[data-model.md](data-model.md) §3）。工具集见 [manager-actions.md](manager-actions.md) 与 [roles-and-tasks.md](roles-and-tasks.md)。

**实现**：

- 使用官方 Rust SDK rmcp 3.5（[architecture.md](architecture.md) §3）。URL 为 `/mcp/{project}/{token}`：路由先按项目 ID 找到打开的项目实例，没有时返回 404；再把项目实例和 token 放进 HTTP 请求的扩展中。工具处理函数从请求上下文读取它们，在这个项目中按 token 找到 turn（data-model.md §10.3）。Angela 的地址是 `/mcp/angela/{token}`，不带项目 ID；token 在她自己的数据库中找到她的 turn，工具参数带项目 ID（data-model.md §10.6）。项目 ID 是带前缀的 ULID，不会是 `angela`。
- 服务不保存 MCP 会话（rmcp 的 stateless 模式），工具调用以单个 JSON 响应返回，不用 SSE 流，不提供 GET 流（返回 405）。调用者已由 URL 中的 token 识别，会话没有额外作用。
- `Host` 只接受 loopback 名称，防止 DNS 重绑定。
- 两家 CLI 协商的协议版本不同（§1.4）：Claude 已使用 2026-07-28 的无状态协议，Codex 仍使用 2025-06-18 的 `initialize` 握手。stateless 模式对两者逐个请求应答，两者都能工作。

**契约测试**：仓库中没有用真实 CLI 的契约测试（[#86](https://github.com/vorton-lang/Lobotomy/issues/86)，用户定）。怎样对照真实 CLI 检查 adapter，随 worker 层的替换再定。

下面的实测结果来自用真实 CLI 的测试，这些测试消耗订阅额度，现在不在仓库中。

MCP 契约测试用真实 CLI 跑一个 turn，确认三点：CLI 调用了工具，处理函数从 URL 取到了 token；所有 HTTP 交换都成功；工具结果到达了模型。测试打印协商的协议版本和每个 HTTP 交换。结果：

- 2026-10-04：Claude Code 2.1.283 与 Codex CLI 0.159.2 都通过。
- 2026-10-10：Claude 的契约测试用适配器拼出的参数（MCP 配置文件、权限、能力裁剪），通过，协商的协议版本为 2026-07-28。

真实 CLI 的后端测试：

- `a_real_codex_turn_reports_done`、`a_real_claude_turn_reports_done`、`a_real_claude_turn_is_interrupted_and_continued`。2026-10-10 都通过：真实 Claude 走完建任务、done、采集、验证到等待验收；输出中没有解析器不认识的事件。
- `a_real_quota_check_passes`：真实 Codex 的额度检查调用。
- `a_real_claude_angela_assigns_a_task`：Angela 在真实 Claude 上派一个只读任务。2026-10-11 通过一次（[roadmap.md](roadmap.md) M4）。
- Claude 的各项用 haiku，结束后删除测试会话的记录。

## 3. 执行现场

| Role | 执行现场 |
|---|---|
| Angela（manager） | host 目录下她实例的 `work/`，一个空目录；按绝对路径读各项目的仓库（data-model.md §10.6） |
| Binah（tech_lead） | 未实现，roadmap 待排 |
| 执行者（worker） | worker 宿主为每个 attempt 准备的工作区：普通文件夹，或给了上游时的一份 clone（worker-interface.md §10） |
| Yesod（reviewer） | 未实现，roadmap 待排。在固定的候选成果上单独物化审查现场的做法依赖私有存储，Lobotomy 没有私有存储（#86） |

后端不运行项目中的 role（[#86](https://github.com/vorton-lang/Lobotomy/issues/86)，用户定）。执行者是 `workers.toml` 中的 worker，在 worker 宿主准备的工作区中运行，见上表。Angela 的 `work/` 是空目录，运行时不把仓库复制进去。

角色定义见 [roles-and-tasks.md](roles-and-tasks.md)。Angela 读项目的仓库，与用户看到同一份代码；她的指令写明仓库只用来展示，不修改（data-model.md §10.6，用户定）。

- **执行现场不是事实来源。** 工作目录只是 CLI 的执行环境，不能反过来推断业务事实；业务事实以 SQLite 为准（#6 §1）。
- **只有 Lobotomy 创建执行现场。** 不使用 `claude -w`、`codex --worktree`，`EnterWorktree` 已在 1.6 中禁用。
- **位置在仓库外**，位于 Lobotomy 的数据目录中，避免被 ripgrep、IDE 索引和测试运行器扫描。

## 4. 成果与验收

运行时不采集、不验证、不发布成果，也不写项目的仓库。依据：成果走 PR、CI 与合并（#86）。执行者报告 `done` 后，那个 turn 结束时，attempt 以候选成果结束，任务进入验收。用户或 Angela 验收后任务完成；退回时开始下一个 attempt（data-model.md §4.1、§9.2）。

进行中的工作在 GUI 中按任务查看，界面见 [frontend.md](frontend.md)：

- Workboard 列出 role 的当前活动与进行中的任务。任务行打开任务面板（frontend.md §7.6）。
- 任务面板显示 worker 的交付：PR、检查结果和交付说明（frontend.md §7.4）。

## 5. 运行时与 Manager 的分工

| 事项 | 负责方 |
|---|---|
| 准备执行现场、对账 | 运行时 |
| 压缩、用量、限流与并发控制 | 运行时 |
| push 与 GitHub 导出 | 单独处理（v1 中 Lobotomy 不 push） |
| GitHub 写操作（未实现，roadmap 待排） | 对外操作通道：role 申请 → Manager 判断 → 用户确认 → 运行时执行 |

## 6. 待验证 / 未决

- 原生中断与中断后的 resume：Codex 与 Claude 在 Windows 上已实测（§1.3 第 6 条、§1.4）。Linux 上的两家仍待实测。结果也决定 ideas.md 中的"中断并发送"能否加入。
- 全局指令文件：role 的 Claude 用 `--setting-sources ""` 启动（§1.6），这时是否仍读取 `~/.claude/CLAUDE.md`，还没有确认。Codex 的做法已定（§1.6）。
- Codex 0.159.2 的 `codex queue`（向已有会话排队一条消息）能否在 `exec` 的 turn 运行中投递消息，待查。若可以，它可能替代 ideas.md 中的"中断并发送"。`codex delete --force <id>` 可以按 ID 删除会话，清理探针或临时会话时使用。
- 输入消息是否进入 harness 的会话记录：turn 在不同时刻中断时，两家 CLI 的会话文件里是否已有本轮输入（data-model.md §3.4）。Claude 额度被拒的情况已有一次记录：输入在报错前写入。
- `-p` stream-json 模式下 Claude 额度被拒的事件形式（data-model.md §8.3）。交互式 CLI 中的形式已有记录；`-p` 下下次自然发生时记录，并核对适配器认的三种信号（§1.4）。Codex 被拒的形式暂不处理。
- Claude 在环境不允许所给权限模式时 stderr 怎么写，还不知道。在此之前，适配器不把它识别为"权限模式不被允许"，turn 按普通失败记录。
- 每个 turn 更换 MCP URL 后，Claude 的跨进程 prompt cache 是否仍命中。Codex 已实测不受影响（§1.4）。
- 平台启动适配（1.8）：Linux 上待实机实测。Windows 已实测；CI 的 Ubuntu 运行进程树测试（`harness/tests/process_tree.rs`）。
- Linux 上重跑实测：[spikes/harness-cli/spike.mjs](../spikes/harness-cli/spike.mjs) 为 Node 脚本，可直接移植。
- 接入方式与条款的调研依据见 [research/harness-interfaces.md](research/harness-interfaces.md)。
- Claude `--append-system-prompt-file`：帮助文本中出现过，尚未实测。Claude 的角色指令现在内联传入，与 Codex 一样受命令行上限约束；指令接近上限时再实测。
- Codex 的长指令方案：`-p` profile 文件，还是 `model_instructions_file`。暂不需要：Windows 命令行上限约 32K 字符，role 指令预计远小于此，先用 `-c developer_instructions`；指令实际接近上限时再研究。
- 权限模式（1.9）：自动审批在真实受管环境中的长期表现，例如联网安装依赖、写工作目录以外的缓存时自动审核的结果与开销；"没有任何输出就退出"能否作为"harness 没有开始这个 turn"的依据，Claude 与 Linux 上待确认；Claude 的自动审批（`--permission-mode auto`、`--permission-prompts none`）在受管环境中的表现还没有实测，本机没有受管环境。
