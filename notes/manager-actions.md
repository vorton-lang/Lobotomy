# Manager 动作集与信息通道

> Status: 设计决定 v0.1（2026-10-01）
> 前置：[harness-adapter.md](harness-adapter.md)（进程模型、执行现场与成果、能力裁剪）

## 0. 原则

- **机械操作归运行时，判断归 Manager。** Manager 是 agent，会犯错；它只出现在需要判断的地方。
- **原文由运行时插入。** Manager 只引用 ID：用户原话、下层产出、记录条目都由运行时原样渲染，Manager 在结构上无法改写或伪造。
- **用户可以质疑系统的判断。** 下层产出全部可达，不删除；Manager 的解读与记录对用户可见、可改；下层可以绕过 Manager 指出解读错误。
- **Manager 就是下层的用户。** Manager 发给下层的消息就是普通的 user turn，Manager 声明自己的身份即可。
- **Manager 可以缺席。** 下文每个动作都对应一条运行时命令，用户在 GUI 中同样可以执行；Angela 不在时，进入她收件箱的内容直接呈现给用户（[roles-and-tasks.md](roles-and-tasks.md) §1.4）。

## 1. 向下：转发原话，解读附在后面

```text
send_to_role(
  role: "tl" | "worker",
  quote: [msg_id, ...],      // 用户消息，运行时按时间顺序原样插入
  context?: [msg_id],        // 可选：用户当时在回复的消息，原样插入
  ledger?: [entry_id],       // 可选：引用记录条目，原样插入
  interpretation: string,    // Manager 的解读，附在后面
)
```

下层收到的内容：

```text
【用户原话】10-01 14:02
这个方向差不多，但是内存模型总觉得有点怪，先别大改。

【用户当时在回复】（Manager 10-01 13:58）
……

【Manager 解读】
……
```

- 转发哪些原话、发给谁，由 Manager 判断；不是每条用户消息都转给所有 role。
- 没有用户原话的消息只有【Manager】一段。段落标题由运行时固定生成，下层始终分得清谁说了什么。
- 解读使用轻量模板，不是严格 schema：我理解的目标 / 我推断的边界 / 我不确定的地方 / 希望何时回报什么。原话以外的内容都属于 Manager 的推断。
- 下层角色指令写明：**原话优先**。原话与解读冲突时以原话为准，并调用 `flag_mismatch` 报告（见 2.3）。
- 用户的消息被转发后，GUI 在该消息下挂一个收起的标签"已转给 Worker（附 Manager 解读）"，点开可核对解读。

## 2. 向上：正文是 Manager 的话，原文作为附件

### 2.1 附件

Manager 的回复就是对用户说的话，其中用 `[[att:<id>]]` 引用下层产出（汇报、turn 输出、检查结果、成果与集成事件、GitHub 条目）。运行时负责：

- 校验 ID 是否存在，伪造的 ID 渲染为错误引用；
- 将附件渲染为**默认收起**的一行，点开才展开；
- 生成附件标题行：来源 role、类型、时间、长度，以及下层在汇报时自己写的标题。Manager 无法给附件写标题。

```text
Worker 的实现已经完成并通过验证，检查全部通过。TL 提了一个问题，我认为值得你现在看：
分配器在并发场景下的语义还没定。
  ▸ TL 汇报 · 并发语义疑点 · 10-01 14:20 · 1.2k 字
```

### 2.2 下层产出的去向

| 状态 | 怎么进入 | 用户在哪里看到 |
|---|---|---|
| 已呈现 | Manager 在回复中引用 | 对话中的附件 |
| 暂缓 | Manager 调用 `defer(ids, reason?)` | Workboard 上的"暂缓 N 条" |
| 未呈现 | 运行时自动归入（没被引用，也没被暂缓） | Inspector |

Manager 不需要逐条表态，只有"暂缓"需要显式调用。

### 2.3 `flag_mismatch`

下层认为 Manager 的解读与用户原话不一致时调用。运行时把它作为**系统提示**直接显示在对话流中，不经 Manager 过滤，因为这是在质疑 Manager 本身。内容是该 role 自己写的说明，原话与解读作为附件附上。Manager 同时收到，以便修正。这条直达通道只用于这一种情况。

## 3. 记录（ledger）

记录由 Lobotomy 保存。Manager 的长期记忆不依赖 harness 自带的记忆，也不依赖原生会话的上下文：harness 和模型会换，它们的上下文窗口和注意力也不同（用户确认，2026-10-09；方向见 [#22](https://github.com/vorton-lang/Lobotomy/issues/22)）。

### 3.1 与运行时状态分开

| 类型 | 内容 | 写入方 |
|---|---|---|
| 运行时状态（事实） | 谁正在跑 turn、成果与集成状态、会话 ID、用量 | 运行时 |
| 记录（判断） | 目标、阶段、决定、约束、待定问题、对用户状态的推测 | Manager，经 MCP 写入 |

### 3.2 结构：只追加，每条带依据

```text
ledger_add(kind, statement, evidence?: [msg_id], confidence?, scope?, supersedes?: entry_id)
```

| kind | 含义 | 依据 |
|---|---|---|
| `goal` / `phase` | 当前目标；阶段（探索 / 设计 / 执行 / 审查） | 可选 |
| `decision` | 已定的事，默认不再重开 | **必须引用用户消息** |
| `constraint` | 约束，标明范围（本任务 / 长期） | **必须引用用户消息** |
| `open_question` | 待定的事，以及在等谁（用户 / TL / Worker） | 可选 |
| `hypothesis` | 对用户状态的推测，附置信度 | 可选 |

- 条目不原地修改，变更时追加新条目并用 `supersedes` 指向旧条目，保留完整的理解演变过程。
- "必须引用用户消息"由运行时校验：ID 必须存在，且必须是用户本人的消息（会议中的发言也算）。没有用户原话作依据的判断，只能作为推测记录。

### 3.3 可见性

- **"共识"面板**：目标、阶段、决定、约束、待定问题。用户可以直接修改、关闭、重开；修改立即生效且为最终结果，由运行时直接写入，Manager 只收到通知，有异议可在之后的回复中提出。
- 等用户回答的 `open_question` 由运行时自动投影到 Workboard 的"等你决定"。
- `hypothesis` 不在主界面显示，只能在 Inspector 中查看。

### 3.4 读者

- **Manager**：一个会话内记录已在上下文中，不每轮重新注入。换会话时，由运行时把当前有效的条目整理后放进新会话的首轮，作为 Manager 的可恢复状态。用户的修改以事件形式通知它。
- **TL / Worker**：通过 `send_to_role` 的 `ledger` 参数获得被引用的条目（原样插入），也可通过只读 MCP 查询 `decision` 和 `constraint`。
- **`hypothesis` 永远不传给下层。**

### 3.5 下层的可恢复状态

TL / Worker 各自通过 `org_checkpoint` 写语义检查点，因为它们最清楚自己的工作状态。写入时机由运行时机械触发：换会话前、Claude 压缩前后。存储机制与 ledger 相同，内容各自独立。

## 4. 升级：会议

### 4.1 机制

会议是运行时维护的共享记录，参会者为用户、Manager 与选定的 TL / Worker。

- 每条发言原样投递给所有参会者，不经转述。
- 参会 role 继续使用自己的主会话（inline）。每轮投递上次以来的新发言，带发言人标签，开头是运行时生成的标题 `【会议】参会者：…`。
- 由 @ 决定谁回复：被 @ 的 role 回复；用户未 @ 任何人时由 Manager（主持人）回复，它可以再 @ 别人。同时被 @ 的 role 并行回复，下一轮才看到彼此的回复。
- agent 之间连续互相 @ 的轮数有上限（默认 3），到达上限后发言权回到用户。由运行时执行。
- 不参会的 role 照常工作，采集与集成照常进行。参会 role 如果正在跑 turn，等这一轮结束再投递。

### 4.2 发起与结束

- 用户点"拉会"并选择参会者。
- Manager 只能 `propose_meeting(participants, agenda, attachments)`，议程中写明"我卡在哪里"，由用户一键接受。下层不能直接发起会议。
- 结束由用户点击，或 Manager 提议、用户确认。结束时运行时要求 Manager 跑一轮收尾：
  1. 写入会议中的决定，依据引用用户在会上的发言；
  2. 写收尾消息，原样投递给所有参会者；
  3. 恢复分层。
- 主对话中留下一张收起的会议卡片："会议 · 14:20–14:45 · TL、Worker · 定了 3 件事"，点开可看完整记录。

### 4.3 其他形式

- **一对一**：在 Inspector 中直接与某个 role 对话，等同于只有用户与该 role 的会议。Manager 不参会，结束后运行时把完整记录作为事件交给 Manager。
- **在官方应用中接管**（待排，见 [roadmap.md](roadmap.md)）：Inspector 中点"在官方应用中打开"后，该 role 标记为"已被用户接管"，暂停投递；点"交还"后恢复。运行时只告诉 Manager 接管发生的时段。
- 如果实际使用中发现会上被否决的方案污染了主会话，再改为 fork 方案：会议在各 role 的 fork 中进行，结束后只把收尾消息带回主会话。

## 5. 对外操作

### 5.1 能力裁剪

所有 role 的对外通道（用户的连接器、会绕开 Lobotomy 的内置工具、git push、gh）在启动参数中去掉，见 harness-adapter.md 1.6。

### 5.2 push

v1 中 Lobotomy 不 push。用户的主仓库是集成版本的只读预览（见 harness-adapter.md §4.3）；push 与 GitHub 导出的内容范围、触发和批准单独处理（#6）。Manager 可以建议导出，但不执行。

### 5.3 外部系统由 Lobotomy 统一接入

Lobotomy 管理项目的全部状态，包括 GitHub 上的状态。外部系统的连接由 Lobotomy 持有，role 经 Lobotomy MCP 访问，权限集中控制。GitHub 是第一个实例，之后的外部系统沿用同一模式。

- **接入方式**：后端调用 `gh` CLI，使用用户自己的登录，Lobotomy 不读取、不保存 token。原则与 harness adapter 相同：官方 CLI、正常用法。role 进程看不到这份登录（见 5.1），凭据只在后端这一层。
- **权限**：按 role × 动作配置 allow / confirm / deny。默认值：

  | 动作 | Manager | TL | Worker |
  |---|---|---|---|
  | 读 issue、PR、review、CI 状态与日志 | allow | allow | allow |
  | 写（建 issue、评论、建 PR、打标签等） | confirm | confirm | confirm |

- **v1 开放的写操作**：只有建 issue 和评论（例如把暂缓的 TL 意见转成 issue）。v1 不 push，因此不需要建 PR。
- **confirm = 对外操作通道**：role 通过 `request_external_action(kind, draft)` 提交完整草稿。下层的申请先由 Manager 判断是否转给用户，Manager 自己的申请直接到用户。用户在 GUI 中看到草稿原文并一键确认，由运行时执行。
- **审计**：每次 GitHub 访问（读和写）都记入事件日志：哪个 role、什么动作、什么时间。
- **项目状态投影**：后端定期轮询（条件请求，304 不计入额度），关注默认分支与相关 PR 的 CI 状态、新 issue、评论、review。产生的事件机械地投影到 Workboard（如 CI 徽章、新评论数），同时进入 Manager 收件箱，由它决定是否在对话中提起。
- **附件快照**：GitHub 条目被引用为附件时，运行时保存引用当时的快照，确保附件内容就是 Manager 当时看到的内容。

## 6. 通知

**只有状态能触发通知。** Manager 没有通知工具：它会犯错，而且"觉得重要就通知"会让通知不断膨胀。Manager 表达紧急只能通过改变状态，即提出阻塞性问题或暂停任务；是否通知由运行时按状态机械判定。

```text
ask_user(question, attachments?, blocking?: [task_id])
  → 生成等待用户的 open_question；blocking 中的任务进入"阻塞"
```

运行时在以下情况发出通知：

| 情况 | 判定 |
|---|---|
| 组织离了用户就无法推进 | 每个 role 要么在等用户（决定、对外操作确认、会议提议），要么无事可做。至少一项在等用户时，是"全员在等你"；队列为空、任务全部完成时，是"活干完了" |
| 系统故障 | harness 登录失效、额度耗尽（`rate_limit_event` 被拒）、CLI 崩溃或版本不兼容、执行结果待对账、预览物化失败 |
| `flag_mismatch` | 工作可能正沿着错误的解读推进，而且这件事不应由 Manager 决定用户是否看到 |

- 其他情况只更新 GUI，包括进度、采集与集成、CI 通过、部分 role 阻塞而其他 role 仍在工作。
- 用户正在看 GUI 时不弹系统通知；同一状态只通知一次，状态变化后才会再次通知；短时间内的多条通知合并。
- 组织空闲时什么都不运行：role 只在收到消息或任务时运行，Manager 只在用户发言或收件箱有新内容时运行。后台不会有 agent 自行扩大范围。

## 7. 工具一览

| 使用方 | 工具 |
|---|---|
| Manager | `send_to_role`、`defer`、`ledger_add`、`ask_user`、`propose_meeting`、GitHub 读工具、`request_external_action`；回复中使用 `[[att:<id>]]` |
| TL / Worker | `org_report(title, body, status)`、`org_checkpoint`、`flag_mismatch`、ledger 只读查询、GitHub 读工具、`request_external_action` |
| 运行时（不是工具） | 采集、集成、预览物化、对账、冲突与检查失败通知、压缩与用量、会议投递与轮数上限、权限校验、附件渲染、GitHub 轮询 |

## 8. 未决

- 解读模板的具体措辞；agent 互相 @ 的轮数上限；快照与暂缓条目的保留期。
- 任务模型与任务相关工具（`assign`、`accept`、`send_back`、`abandon`）见 [roles-and-tasks.md](roles-and-tasks.md)。
