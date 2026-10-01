# Harness Adapter、Worktree 与同步

> Status: 设计决定 v0.1（2026-10-01）
> 实测环境：Windows 11，Claude Code 2.1.283，Codex CLI 0.159.2（随 Codex desktop app 分发）

## 0. 原则

- **机械操作归运行时，判断归 Manager。** Manager 是 agent，会犯错；能用确定性规则完成的事不经过它，它只收到事件用于知情。
- **只按正常方式调用官方 CLI。** 不依赖 Claude Agent SDK、Codex SDK / app-server 或 ACP adapter。订阅绑定官方入口，正常调用 CLI 与用户手动使用一致。CLI 做不到的，由 Lobotomy 的 MCP 服务提供。
- **不依赖 harness 的权限系统。** 所有 role 关闭审批。安全靠结构：git 可恢复、守护进程与开发副本分离、对外操作单独约定。
- **harness 自己的日志和会话文件不管理。**

## 1. Adapter

### 1.1 进程模型：每轮一个进程

每个 turn 启动一次 CLI，结束即退出；跨 turn 的连续性完全靠 harness 原生的 resume。

- 与用户手动执行命令相同；
- 守护进程重启后不需要额外恢复逻辑，下一轮照常 resume；
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

1. **每轮都在 role 的工作目录启动进程。** 两家 resume 都不恢复 cwd，turn 在进程 cwd 中执行。
2. **每轮都传角色指令与 MCP 配置。** Claude 在压缩后按当轮进程参数重新生成并固定 system prompt；压缩后首轮若未传 `--append-system-prompt`，角色指令会从 system prompt 中永久消失，之后再传也被忽略，直到下一次压缩。模型仍能从压缩摘要中说出角色名，所以问题不显眼。Codex 不需要重传，传了也无害。system prompt 固定后重复传参不影响缓存。
3. **消息走 stdin，长配置走文件。** Windows 命令行上限约 32K 字符。Codex 在 stdin 非 TTY 时会读取 stdin 并追加为 `<stdin>` 块，必须显式提供或关闭 stdin。Claude 的 `--mcp-config` 接收多个值，会把其后的位置参数（提示词）当作配置文件路径吞掉（实测踩到）。
4. **压缩：** Claude 从 stdout 的 `system/compact_boundary` 读取（含 trigger、压缩前后 token 数）。Codex 的压缩设计上不影响工作，不做追踪；可尽力读 rollout 文件中的 `compacted` 记录，仅用于 GUI 显示。
5. **用量：** Claude 每轮 `result.usage`，另有 `rate_limit_event`（5 小时 / 7 天额度利用率与重置时间），用于运行时的并发控制和 GUI 显示。Codex 的 `turn.completed.usage` 实测为线程累计值，需与上一轮做差。
6. **中断 = 结束进程树，下一轮 resume。** Claude 经 scoop shim 启动，必须结束整棵进程树。
7. **事件解析要容错。** 两家 JSON 事件格式都不在稳定承诺内：未知事件忽略，并记录原文。

### 1.4 实测结果

| 项目 | Claude Code | Codex |
|---|---|---|
| 每轮单进程 | 首个输出约 2s，简单一轮 4–5s（haiku） | 首轮首个输出约 2.7s，resume 约 0.2s，一轮 7–22s |
| 跨进程 prompt cache | 命中约 23.6k token，1h TTL | 输入大部分来自缓存 |
| HTTP MCP 注入 | `--mcp-config` 可用，每轮重新 initialize | `-c mcp_servers.<name>.url=` 可用 |
| 角色指令 | `--append-system-prompt` 可用，压缩后须重传 | `-c developer_instructions=` 可用，resume、压缩后保留 |
| resume 时的 cwd | 不恢复 | 不恢复 |
| 压缩事件 | stdout 有；`-p "/compact"` 可手动触发 | 压缩会发生，但 `--json` 输出无事件，仅 rollout 文件有 |
| fork | 可用，返回新 id | 可用，返回新 id |

### 1.5 Codex 二进制定位

Codex 不在 PATH 上，位于 `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe`。hash 目录随 app 更新变化，版本也可能在运行期间升级。adapter 需自动探测并允许手动配置，每次启动记录版本。

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
- 外部系统（GitHub 等）由 Lobotomy 统一接入，role 经 Lobotomy MCP 访问，见 `manager-actions.md`。

## 2. MCP 服务

守护进程在 localhost 提供 streamable HTTP MCP。每个 role 分配一个带 token 的 URL，由 URL 识别调用者，每轮不需要额外启动子进程。工具集见 Manager 动作集（待定）。

## 3. Role 与工作目录（v1）

| Role | 工作目录 | 分支 |
|---|---|---|
| Angela（manager） | 用户的主仓库（按指令只读） | — |
| Binah（tech_lead） | 槽位 `tl` | `lobotomy/tl` |
| Malkuth（worker） | 槽位 `worker` | `lobotomy/worker` |
| Yesod（reviewer） | 审查期间使用 `worker` 槽位（只读） | — |

Manager 与用户看到的是同一份代码。角色定义见 [roles-and-tasks.md](roles-and-tasks.md)。

### 3.1 槽位

- **只有 Lobotomy 创建 worktree。** 不使用 `claude -w`、`codex --worktree`；角色指令中要求 role 不自行创建。
- **位置在仓库外：** `%LOCALAPPDATA%\Lobotomy\worktrees\<repo-id>\<slot>`，避免被 ripgrep、IDE 索引和测试运行器扫描。
- **槽位固定、复用：** 一次创建，长期使用，磁盘占用有结构性上限。重置时用 `git reset --hard` + `git clean -fd`，不加 `-x`，保留被忽略的依赖和构建缓存。
- **工作成果存在 git ref 里，而不是目录里：**
  - 每轮开始前用 `git stash create` 快照，存为 `refs/lobotomy/snapshots/<slot>/<turn>`，支持"撤销这一轮"；
  - 任务放弃时未同步的提交存为 `refs/lobotomy/parked/<task-id>`；
  - 按保留期清理。
- **对账：** 启动时与定期执行 `git worktree prune`，对齐登记表与磁盘，只处理 Lobotomy 路径下的内容；非 Lobotomy 创建的 worktree 只报告，不删除。

## 4. 同步

工作越早回到用户视野越好：worktree 里的东西用户看不见。

### 4.1 定义

- **目标分支**：主仓库当前检出的分支。槽位从它分出，也合回它。
- **同步**：把槽位的提交并入目标分支，并更新用户的工作区。只在本地进行，**不包括 push**。
- **同步单位是 commit。** Worker 每完成一个连贯的小步就提交（写进角色指令）。未提交的改动不同步，由 GUI 显示和快照兜底。

### 4.2 流程

由运行时执行。每个新提交都触发，以检查结果作为门槛：

```text
role 一轮结束，有新提交
 ① 槽位内 rebase 到目标分支最新
      冲突 → abort，运行时把冲突信息直接发给该 role，下一轮处理
 ② 运行仓库配置的检查命令（未配置则跳过）
      失败 → 运行时把输出直接发给该 role
 ③ 主仓库内 git merge --ff-only lobotomy/<slot>
      与用户未提交修改重叠 → git 拒绝 → 状态"被本地修改挡住"，稍后重试
 ④ 记录同步事件（前后 SHA），Manager 收到事件，仅用于知情
```

运行时发给 role 的消息使用固定模板，并标明来源是运行时。

### 4.3 安全原则

- 目标分支只快进，从不改写；需要改写的只有槽位分支。
- 不碰用户未提交的修改：依靠 git 自身对 `--ff-only` 的检查。
- 撤销同步用 `git revert`，不用 `reset`。
- 用户在目标分支上的提交，下次 rebase 时自动带入槽位。
- 主仓库切换分支后，运行时暂停同步，并在 GUI 显示。
- `index.lock` 冲突时退避重试。

### 4.4 TL 槽位

TL 槽位使用同一机制。每轮开始前，若没有未同步的提交和未提交的改动，就移到目标分支最新提交，保证 TL 审查的是刚同步的代码。TL 的提交走同一同步流程。

### 4.5 GUI 可见性

Workboard 按槽位显示：

- 领先 / 落后的提交数、未提交文件数；
- 同步状态：已同步 / 检查中 / 冲突处理中 / 被本地修改挡住 / 已暂停；
- 点开显示 diff。

## 5. 运行时与 Manager 的分工

| 事项 | 负责方 |
|---|---|
| 槽位分配、重置、快照、对账 | 运行时 |
| rebase、检查、快进、撤销同步 | 运行时 |
| 冲突与检查失败通知 role | 运行时（模板消息） |
| 压缩、用量、限流与并发控制 | 运行时 |
| 主仓库切分支时暂停同步 | 运行时 |
| 因方向重议而暂停同步 | Manager（可选） |
| push | 用户自己（v1 中 Lobotomy 不 push） |
| GitHub 写操作 | 对外操作通道：role 申请 → Manager 判断 → 用户确认 → 运行时执行 |

## 6. 待验证 / 未决

- Claude `--append-system-prompt-file`：帮助文本中出现过，尚未实测。
- Codex 的长指令方案：`-p` profile 文件，还是 `model_instructions_file`。
- Codex `--thread-source` 的取值。
- Windows 上结束进程树后，被中断 turn 在两家会话记录中的状态。
- `--ignore-user-config` 下全局 `AGENTS.md` 是否仍生效。
