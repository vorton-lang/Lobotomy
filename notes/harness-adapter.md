# Harness Adapter、执行现场与成果

> Status: 设计决定 v0.2（2026-10-02）
> v0.1 的 git worktree + rebase + 快进同步方案已由 [#5](https://github.com/vorton-lang/Lobotomy/issues/5) 的已确认评论与 [#6](https://github.com/vorton-lang/Lobotomy/issues/6) 的推荐实现取代：SQLite 是业务事实的唯一权威，jj 承载不可变成果，工作目录只是执行现场。
> 实测环境：Windows 11，Claude Code 2.1.283，Codex CLI 0.159.2（随 Codex desktop app 分发）。

## 0. 原则

- **机械操作归运行时，判断归 Manager。** Manager 是 agent，会犯错；能用确定性规则完成的事不经过它，它只收到事件用于知情。
- **只按正常方式调用官方 CLI。** 不依赖 Claude Agent SDK、Codex SDK / app-server 或 ACP adapter。订阅绑定官方入口，正常调用 CLI 与用户手动使用一致。CLI 做不到的，由 Lobotomy 的 MCP 服务提供。
- **不依赖平台特有行为。** 能力取 Windows 与 Linux 的交集。先在 Windows 上开发运行，正式发版前集中修 Linux 问题。下文标为"Windows 实测"的细节属于平台适配层，不是设计依赖。
- **不依赖 harness 的权限系统。** 所有 role 关闭审批。安全靠结构：成果采集可恢复、守护进程与开发副本分离、对外能力裁剪。
- **harness 自己的日志和会话文件不管理。**

## 1. Adapter

### 1.1 进程模型：每轮一个进程

每个 turn 启动一次 CLI，结束即退出；跨 turn 的连续性完全靠 harness 原生的 resume。

- 与用户手动执行命令相同；
- 守护进程重启后，下一轮照常 resume；上一轮结果不明时按对账处理（见 1.3 第 6 条）；
- 两家 CLI 形状一致，不需要监管长驻子进程；
- prompt cache 在服务端按前缀命中，与进程是否常驻无关（实测命中）。

代价是每轮约 2 秒启动开销，以及 turn 进行中不能插话。发给 role 的消息先进入它的收件箱，在当前 turn 结束后投递。

### 1.2 调用形式

```text
Claude（消息经 stdin）
  首轮：claude -p --session-id <uuid> --output-format stream-json --verbose
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
2. **每轮都传角色指令与 MCP 配置。** Claude 在压缩后按当轮进程参数重新生成并固定 system prompt；压缩后首轮若未传 `--append-system-prompt`，角色指令会从 system prompt 中永久消失，之后再传也被忽略，直到下一次压缩。模型仍能从压缩摘要中说出角色名，所以问题不显眼。Codex 不需要重传，传了也无害。system prompt 固定后重复传参不影响缓存。
3. **消息走 stdin，长配置走文件。** 命令行长度有平台上限（Windows 约 32K 字符）。Codex 在 stdin 非 TTY 时会读取 stdin 并追加为 `<stdin>` 块，必须显式提供或关闭 stdin。Claude 的 `--mcp-config` 接收多个值，会把其后的位置参数（提示词）当作配置文件路径吞掉（实测踩到）。
4. **压缩：** Claude 从 stdout 的 `system/compact_boundary` 读取（含 trigger、压缩前后 token 数）。Codex 的压缩设计上不影响工作，不做追踪；可尽力读 rollout 文件中的 `compacted` 记录，仅用于 GUI 显示。
5. **用量：** Claude 每轮 `result.usage`，另有 `rate_limit_event`（5 小时 / 7 天额度利用率与重置时间），用于运行时的并发控制和 GUI 显示。Codex 的 `turn.completed.usage` 实测为线程累计值，需与上一轮做差。额度作用域与降级见 [#4](https://github.com/vorton-lang/Lobotomy/issues/4)。
6. **停止与中断（#6 §3）：** 复用 harness 原生的 interrupt / graceful exit 并等待 CLI 退出；假定官方 harness 自行清理子进程，不自建进程树管理。正常停止只 drain 当前 turn；立即中断走原生中断路径。CLI 未退出或执行状态不明时保持"待对账"，不重放（[#2](https://github.com/vorton-lang/Lobotomy/issues/2)）。
7. **事件解析要容错。** 两家 JSON 事件格式都不在稳定承诺内：未知事件忽略，并记录原文。

### 1.4 实测结果（Windows）

| 项目 | Claude Code | Codex |
|---|---|---|
| 每轮单进程 | 首个输出约 2s，简单一轮 4–5s（haiku） | 首轮首个输出约 2.7s，resume 约 0.2s，一轮 7–22s |
| 跨进程 prompt cache | 命中约 23.6k token，1h TTL | 输入大部分来自缓存 |
| HTTP MCP 注入 | `--mcp-config` 可用，每轮重新 initialize | `-c mcp_servers.<name>.url=` 可用 |
| 角色指令 | `--append-system-prompt` 可用，压缩后须重传 | `-c developer_instructions=` 可用，resume、压缩后保留 |
| resume 时的 cwd | 不恢复 | 不恢复 |
| 压缩事件 | stdout 有；`-p "/compact"` 可手动触发 | 压缩会发生，但 `--json` 输出无事件，仅 rollout 文件有 |
| fork | 可用，返回新 id | 可用，返回新 id |

### 1.5 二进制定位

adapter 按平台自动探测两家 CLI，并允许手动配置；每次启动记录版本。Windows 实测：Claude 经 scoop shim 位于 PATH；Codex 不在 PATH 上，位于 `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe`，hash 目录随 desktop app 更新变化，版本可能在运行期间升级。

### 1.6 能力裁剪

role 会话默认继承用户的全部对外通道：Claude 继承 claude.ai 连接器（Docs、Slack、Drive），Codex 继承 ChatGPT 账号的 apps 连接器（GitHub 写操作、Outlook 发信、Drive 分享、Sites 部署等），以及 computer-use 和浏览器控制。这些对写代码没有用，却是以用户身份对外发送内容的主要渠道。做法不是加审批，而是在启动参数中去掉能力，零延迟、零 token。对所有 role 生效，Manager 也包括在内。

| 能力 | Claude | Codex |
|---|---|---|
| 用户的连接器 | `--strict-mcp-config`（实测只剩 Lobotomy MCP） | `--ignore-user-config --disable apps --disable computer_use --disable browser_use`（实测只剩执行、改文件、网页搜索、生图、子 agent） |
| 绕开 Lobotomy 的内置工具 | `--disallowed-tools CronCreate CronDelete CronList ScheduleWakeup RemoteTrigger PushNotification SendMessage ListAgents EnterWorktree ExitWorktree DesignSync` | — |
| git push | 进程环境变量 `GIT_CONFIG_*` 注入 `url.lobotomy-push-disabled://.pushInsteadOf`（`https://`、`git@`、`ssh://`）；实测 push 在本地失败，fetch 不受影响 | 同左 |
| gh | `GH_CONFIG_DIR` 指向空目录，等同未登录 | 同左 |

- 保留网页搜索与读取、子 agent。
- 这些措施防的是失误和过度热心，不防对抗：bypass 模式下 agent 总能绕过（例如从凭据管理器取 token）。
- 运行时每轮从 Claude 的 `system/init` 事件读取工具清单，与已知清单比对；CLI 升级后出现的新工具在 Inspector 中标出，由用户决定是否禁用。
- Codex role 不读用户的 `config.toml`，模型、推理强度等由 Lobotomy 显式传入。
- 外部系统（GitHub 等）由 Lobotomy 统一接入，role 经 Lobotomy MCP 访问，见 [manager-actions.md](manager-actions.md)。

## 2. MCP 服务

守护进程在 localhost 提供 streamable HTTP MCP。每个 role 分配一个带 token 的 URL，由 URL 识别调用者，每轮不需要额外启动子进程。工具集见 [manager-actions.md](manager-actions.md) 与 [roles-and-tasks.md](roles-and-tasks.md)。

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
- **槽位固定、复用**，磁盘占用有结构性上限。被忽略的依赖与构建缓存在重新物化时保留。
- **物化**：每个槽位是一份独立的 git clone，对象经 alternates 与私有存储共享，HEAD 设为本轮基线，使 agent 的 `git status` / `git diff` / `git log` 正好反映本轮工作。不用 linked worktree：它与主库共享 ref，agent 的 `git branch -D`、`git gc` 等操作可能删掉成果的保留 ref。agent 在 clone 中的提交只是草稿，不是同步单位。
- **重新物化**：采集完成后才可复用槽位；checkout 中断或就绪状态不明时，隔离旧现场，以新 generation 重新物化（#6 §2）。
- **审查现场**：Reviewer 不借用 Worker 的槽位，而是在固定的候选成果上单独物化一份现场。成果不可变，Reviewer 在现场中的任何改动都不会进入成果，因此不需要审查前后比对与恢复，也消除了与 Worker 槽位的占用冲突（[#1](https://github.com/vorton-lang/Lobotomy/issues/1)）。现场随任务关闭回收。

## 4. 成果与集成

### 4.1 采集与发布（#6 §2）

```text
① SQL intent    持久保存操作身份、task / attempt、条件版本、预期 revision、现场 generation、采集范围
② jj capture    确认写者已停止后，按声明范围采集文件树，固定 commit_id 并建立 pin
③ SQL publish   短事务发布成果引用，校验 revision 与 expected-current，连同 outbox 一并提交
```

- 已有 pin 时，恢复与重试始终使用同一份成果；没有 pin 时保持 pending，确认写者停止后再决定是否采集。
- 迟到或过期的成果保留来源，但不能推进当前状态。
- **采集范围由运行时声明**，不只依赖 harness 可修改的 ignore 文件（`force_tracking_matcher`）。特殊文件、嵌套仓库等 fail closed：报告未覆盖的内容并保留现场（[#3](https://github.com/vorton-lang/Lobotomy/issues/3)）。
- 每轮采集取代了 v0.1 的 `git stash create` 快照，"撤销这一轮"基于采集记录实现。

### 4.2 集成：只发布已验收的成果

集成版本只沿一个方向前进，只接收已验收的成果。回滚不会出现在控制面中。

- **验证阶段**：运行时用 jj 把候选成果 rebase 到当前集成版本上。jj 把冲突作为数据记录在提交中，rebase 不会中断；有冲突时以运行时模板消息交还给相应 role 处理。检查命令在 rebase 后的结果上运行。
- **证据绑定**：检查、审查与验收的证据都绑定到具体成果、基线与条件版本（[#5](https://github.com/vorton-lang/Lobotomy/issues/5) 已确认的 TASK 方向）。
- **验收即发布**：发布是对集成版本头的 CAS。若集成版本在验证之后已经前进，运行时把候选成果 rebase 到新的头并重新验证，必要时重新审查；不能凭旧证据发布。
- **未验收的候选成果不进入预览，也不能被其他任务依赖。** 需要用到它的任务排在它之后。
- **撤销已发布的代码**只能作为新任务向前发布，例如一个 revert 成果，同样经过验证、审查与验收。

### 4.3 主仓库：集成版本的只读预览

- 用户的主仓库是集成版本的**单向物化**，由运行时机械写入，不是任何事实的来源。只包含已验收的成果。
- 前提是用户不手工修改项目文件（#5）。Angela 在这里只读运行，与用户看到同一份代码。
- 进行中的工作通过 GUI 查看（Inspector 的"改动"页：按执行轮的成果与 diff），不进入预览。
- push 与 GitHub 导出的内容范围、触发和批准单独处理，v1 中 Lobotomy 不 push。

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

- jj-lib 的版本锁定与封装边界：jj-lib 的库 API 尚未稳定，需锁定版本，并封装在一个模块后面，不让 jj 的类型扩散到业务代码。
- 跨平台的原生中断：per-turn 进程在 Windows 与 Linux 上如何一致地触发 harness 原生中断，被中断 turn 在两家会话记录中的状态。
- Linux 上重跑实测：[spikes/harness-cli/spike.mjs](../spikes/harness-cli/spike.mjs) 为 Node 脚本，可直接移植。
- 接入方式与条款的调研依据见 [research/harness-interfaces.md](research/harness-interfaces.md)。
- Claude `--append-system-prompt-file`：帮助文本中出现过，尚未实测。
- Codex 的长指令方案：`-p` profile 文件，还是 `model_instructions_file`。
- Codex `--thread-source` 的取值。
- `--ignore-user-config` 下全局 `AGENTS.md` 是否仍生效。
