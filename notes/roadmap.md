# Roadmap

> Status: 规划 v0.3（2026-10-10）
> v0.1 的编号：M2 审查、M3 Binah 与会议（含 Claude adapter）、M4 Angela。旧 issue 中的编号按此对应。

## 原则

- **每个里程碑都端到端可用。** 先打通一条完整链路，再增加对象种类和调度能力。
- **主路径先存在。** 主路径是用户经 Angela 管理项目，借鉴 OpenAI Dots。用户也可以直接打开 Malkuth 等角色的窗口。这条直接通路是常设的二等入口，不是临时方案；用户自然选择，不强制。M1 已经验证"没有 Angela，控制流也能跑通"。
- **接触成果是一等入口，直接指挥执行者是二等入口。** 用户在任何地方都能直接查看和试用成果。Angela 对话中引用的成果，点开就是任务面板中同一个 diff 与试用视图。二等入口指的是另一件事：绕过 Angela，直接给执行者发消息或建任务。依据：[#20](https://github.com/vorton-lang/Lobotomy/issues/20) 约束 5 要求用户能直接接触成果；Dots 式的主路径约束的是指挥执行者，不是查看成果（用户确认，2026-10-11）。
- **事实、权威、判断分开。** 运行时持有事实，用户持有权威，Angela 与各 role 给出判断。判断不能冒充事实，也不能冒充权威。现有的例子：原文由运行时插入，Manager 只引用 ID；`decision` 必须引用用户原话；只有状态能触发通知；记忆不能给模型扩权（[#22](https://github.com/vorton-lang/Lobotomy/issues/22) §4）。新设计按这一条检查：事实归谁，权威归谁，判断有没有机会伪装成另外两者（用户确认，2026-10-11）。
- **角色不绑定 harness。** 每个 role 都能换 harness，Angela 也一样。外部测试者没有 Claude，所有 role 都要能在 Codex 上运行。官方 CLI 担任 Angela 表现不佳时，可以改用自建的 API harness（用户确认，2026-10-09）。
- **长期记忆由 Lobotomy 保存。** 不依赖 harness 自带的记忆或原生会话的上下文：harness 和模型会换，它们的上下文窗口和注意力也不同（用户确认，2026-10-09）。
- **性能基线从 M1 开始**（[frontend.md](frontend.md) §5）。
- 在 Windows 上开发，不依赖平台特有行为；正式发版前集中修 Linux 问题。

## 里程碑

| 里程碑 | 内容 | 状态 |
|---|---|---|
| **M1 执行闭环** | 用户在 GUI 中给 Malkuth 建任务 → 执行 → 采集并固定成果 → 验证 → 用户验收 → 发布 → 物化预览 | 完成。用户与测试者在 Windows 和 Linux 上用真实 Codex 走通全链路。设计见 [data-model.md](data-model.md)、[harness-adapter.md](harness-adapter.md)、[frontend.md](frontend.md) |
| **M2 第二个 harness** | Claude adapter | 完成，2026-10-10。设计与实测见 harness-adapter.md §1.2–§1.4，没有验证的事项见 §6 |
| **M3 多项目** | 一个后端管理所有项目；项目列表、新建、归档与取消归档；全局汇总的"等你决定" | 完成，2026-10-11，用户用两个真实仓库验收。设计见 data-model.md §10、frontend.md §1–§3、harness-adapter.md §2 |
| **M4 最小的全局 Angela** | 见下 | 第 1–3 步已实现（2026-10-11），等用户在实际使用中验收 |
| **界面与交互的改进专项** | 视觉基础、角色与状态、对话、"等你决定"、任务面板、窗口与导航、回来时的接续。只改前端 | 进行中，与 M4 的验收并行（用户确认，2026-10-12）。P0、P1 已完成（2026-10-12），下一步是 P2。计划见 [ui-ux-plan.md](ui-ux-plan.md) |

### M4 最小的全局 Angela

**第一优先级是降低用户回来接上工作的成本**（[#50](https://github.com/vorton-lang/Lobotomy/issues/50)，用户确认，2026-10-11）。用户回来时，应能直接试用成果、看懂关键变化、表达下一步意见，不必从聊天记录重新讲起。怎样帮助用户回来后进入状态，用户状态不确定时怎样协作，是开放的研究问题（[#60](https://github.com/vorton-lang/Lobotomy/issues/60)，关联 [#21](https://github.com/vorton-lang/Lobotomy/issues/21)、#50），不作为立即实现的方案。

- 全局只有一个 Angela，放在 host（data-model.md §10.6）。她能看到所有未归档的项目，工具都带项目 ID。
- 主区是与 Angela 的对话。各项目中 Malkuth 的对话、任务面板和"等你决定"保留为直接入口。Angela 引用的成果，点开就是任务面板中同一个 diff 与试用视图（见"原则"）。
- 她的工具：
  - 读取：`list_projects`、`read_project`、`read_task`、`read_events`、`read_diff`；
  - 操作：`assign`、`send_to_role`、`accept`、`send_back`、`abandon`、`set_paused`；
  - 提问与记录：`ask_user`、`ledger_add`、`read_ledger`、`recent_replies`；
  - 回复中用 `[[quote:…]]` 引用用户原话，用 `[[att:…]]` 引用附件（[manager-actions.md](manager-actions.md) §1、§2）。

  `assign` 与 `send_to_role` 只限 Manager；用户在 GUI 中建任务、发消息，走原来的命令（data-model.md §10.6 跨库操作第 2 条）。`assign` 必须写明允许的动作和停止条件（[roles-and-tasks.md](roles-and-tasks.md) §4）。
- Angela 的指令（Manager 的思维方式、决定下一步的程序、派活与上行过滤的规则、难例）在 [manager.md](../backend/crates/lobotomyd/src/prompts/manager.md) 中，改自 Ember 的[草稿](https://github.com/vorton-lang/Ember/blob/main/manager-interviews/manager-draft.md)。指令按真实使用中的失败迭代，这些失败也作为面试的新案例（hold-out）（**语义更新**，用户确认，2026-10-11）。
  - 依据：#50 的检验标准是用户少重述、少追问、少返工，这些只能在自然协作中观察；第三轮面试中，指令中的语义规则没有守住（[Ember 第三轮](https://github.com/vorton-lang/Ember/tree/main/manager-interviews/2026-10-11-official-cli-round3)）。所以 #50 中能做成结构的原则做进运行时，见下面关于暂停、读取边界和附件的三条。
  - 每次失败按 #60 的写法记下：场景、期望的行为、实际怎样失败、改哪一层能修复（指令、运行时或模型）。凭一次成功不算修好，要用没见过的对照场景再检验（#60）。
- 先不定 Angela 用哪个模型（用户判断，2026-10-11）。依据：两轮面试中，候选之间没有本质差距，起作用的主要是指令、环境、记忆和约束；这些搭完整后，不同模型的操作空间会收束。以后再找默认取向正好对齐这个岗位的模型。
- 记录（ledger）只做最小的一版：目标、决定、约束带用户原话作依据，推测单独存放；开新会话时投影进首轮输入，也可经 MCP 查询（manager-actions.md §3）。
- 用户直接操作项目后，Angela 从事件中得知变化，不要求用户重述，也不重复旧操作（#23 §3）。用户设的暂停只有用户能解除（roles-and-tasks.md §4）。
- Angela 的读取结果写明边界：每个项目读到哪里为止，哪些项目没有读到，每条结果是否仍然有效（data-model.md §10.6）。
- 附件写明它绑定的版本，运行时按当前状态标出它是否已过期（manager-actions.md §2.1）。
- 收到模糊反馈时，Angela 不把理解的负担推回给用户，先在已授权范围内调查或做小实验，再带结果回来（#21）。这是对她行为的检查项，不是要统计的指标。
- 实现分 3 步，每步做完都能使用。三步都已完成（2026-10-11）：
  1. **能对话、能读。** 她的数据库与 turn 在 host 目录下；「全部」的主区是与她的对话，所有项目的"等你决定"在右侧栏；她经 MCP 读所有项目，读取结果带边界（data-model.md §10.6）。设置中可以切换她的 harness。
  2. **能操作。** 调用者 `manager`；派活、转发原话、验收、退回、放弃、暂停；回复中的引用由运行时渲染，附件带版本与当前有效性（manager-actions.md §2.1）。任务面板显示她派活时写的解读、允许的动作和停止条件，并写明任务是谁暂停的。只读调查的任务产生了文件改动时，运行时标出（roles-and-tasks.md §4）。
  3. **跟上变化。** 收件箱与唤醒、`ask_user`（manager-actions.md §6）；最小的记录与换会话时的投影，`ledger` 参数（manager-actions.md §3.4）；`flag_mismatch`（manager-actions.md §2.3）；`context`、用户手动指定回复对象、"已转给 …"标签（manager-actions.md §1.1）。
- 验收之前补上三项（用户确认，2026-10-13，[#60](https://github.com/vorton-lang/Lobotomy/issues/60)）：
  - 她的每条输入开头写明用户的本地时间（data-model.md §10.6"她的输入"）。指令中的"时间与节奏"把它当作弱信号。
  - 她用 `read_diff` 读候选成果与验证的 diff（data-model.md §10.6）。
  - 用户在设置中为每个 role 选模型和推理强度，两个 harness 各存一份（harness-adapter.md §1.2）。

  依据：验收要观察她能否先查证、再判断。她读不到 diff 时，判断出错就分不清是判断的问题，还是缺信息的问题。第三项来自验收前的试用：没有选择时，Claude role 用 CLI 默认的 Fable，Fable 的额度用完后她停了；Codex role 用默认的 `gpt-6.1-sol` 和 low，一次回复要几分钟（harness-adapter.md §1.2 的实测）。这一项不改"先不定 Angela 用哪个模型"：Lobotomy 不设默认模型，由用户选。
- 设计了而 M4 没有实现的部分，列在下面"待排"的"Angela 与记录"一组。
- 实测（2026-10-11，Windows，Claude Code，haiku）：用真实 CLI 走过的只有第 2 步的 `assign`。她先读项目，再调用 `assign`，`actions`、`quote`、`stop_when` 都按工具的参数说明填写，引用的是用户那条消息的 ID。测试是 `a_real_claude_angela_assigns_a_task`，默认忽略。这只验证了一次调用，没有验证她在自然协作中的判断。第 3 步的行为只用假 CLI 测过。
- #50 的三组开放问题属于 Angela 的判断：怎样估计用户的状态，怎样选择行动与介入时机，给出什么信息。M4 不回答这些问题，只搭出能观察它们的环境。M4 不做用户状态模型、信息价值的打分、为对照而做的多个版本，也不改通知规则。需要提前告诉用户的变化，Angela 按现有规则改变状态：用 `ask_user` 提问，要停下的任务另用 `set_paused` 暂停（manager-actions.md §6）。系统通知还没有实现，见"待排"。
- 验收：
  - 用户只与 Angela 对话，完成一个真实任务。#60 建议先验证的方向并入这一条（用户确认，2026-10-13）：Angela 独立负责一个小范围的问题，调查、给出判断和建议，在授权内推进，再带回可核查的结果。#60 的其余部分仍是研究问题；
  - 用户离开一段时间后回来，Angela 的消息给出可运行的入口、关键变化、同一版本的验证结果和真正待决的分叉，用户不重述就能给出下一步意见。从自然协作中观察重述、追问和返工，不加评价表（#50）；
  - 用户直接暂停任务或修改完成条件后，Angela 按新状态继续；
  - 查询有缺口时，Angela 讲清缺口：不把没有读到说成没有，也不把过期的结果当作当前事实；
  - Angela 改用 Codex 后仍然可用。

M4 之后，用真实项目使用 Lobotomy，包括开发它自己，再按实际痛点从下面的待排事项中排序。界面与交互的改进专项是例外：它已在进行，与 M4 的验收并行（用户确认，2026-10-12）。

### #20 的约束在哪里检验

| 约束 | 检验 |
|---|---|
| 1 先提供低成本的真实体验 | 已有：交付说明与候选成果试用（frontend.md §8）。M4 中 Angela 用已有的 diff、报告与试用；用户回来时，她的消息给出可运行的入口（#50）；试用的改进待排 |
| 2 反馈影响后续自主选择 | M4：ledger 中的决定与约束进入后续任务说明 |
| 3 保留自主展开的空间 | M4：Angela 的行为检查 |
| 4 关键变化时重新共同判断 | M4：`ask_user` 只用于关键取舍 |
| 5 用户能直接接触成果 | M1 已有；M4 中 Angela 引用的成果打开同一个 diff 与试用视图，项目中的直接入口保留 |
| 6 共识是可修订的工作依据 | M4：原话与推测分开，变更用 `supersedes` |

## 待排

下面是已经设计、还没有实现的事项，以及决定要做、还没有排期的事项。按主题分组，组内与组间的顺序都不表示先后。设计写在所注的文件与小节中；已写成设计的部分，正文标"未实现"。

### 审查与会议

| 事项 | 设计与来源 |
|---|---|
| 审查：Yesod、独立审查现场、审查结论；一轮返工预算与结论失效规则 | roles-and-tasks.md §1.3、§2.2；harness-adapter.md §3；data-model.md §9.3；v0.1 的 M2；[#19](https://github.com/vorton-lang/Lobotomy/issues/19) |
| Binah 与会议；会议与 Thread 的关系；研究任务按非代码证据完成 | roles-and-tasks.md §1.2、§1.3；manager-actions.md §4；data-model.md §7.1、§9.3；v0.1 的 M3；[#5](https://github.com/vorton-lang/Lobotomy/issues/5) |

### Angela 与记录

| 事项 | 设计与来源 |
|---|---|
| `defer`（暂缓），以及 Workboard 上的"暂缓 N 条" | manager-actions.md §2.2 |
| `propose_meeting`，随会议一起做 | manager-actions.md §4.2 |
| `ask_user` 与 `flag_mismatch` 带附件 | manager-actions.md §2.3、§6 |
| `assign` 的 `links`，等 GitHub 接入 | roles-and-tasks.md §4 |
| "共识"可修改：用户直接修改、关闭、重开条目 | manager-actions.md §3.3、§3.4；roles-and-tasks.md §1.4 |
| 下层对记录的只读查询 | manager-actions.md §3.4 |
| 用户纠正记录后，把更新送到受影响的活跃任务 | #22 §2 |
| 下层的语义检查点 `org_checkpoint` | manager-actions.md §3.5 |
| 附件引用 turn 输出、集成事件与 GitHub 条目 | manager-actions.md §2.1 |

### 通知

| 事项 | 设计与来源 |
|---|---|
| 系统通知：后端按状态判定并保存通知意图，Electron 主进程投递；等 Angela 写完合并的消息再发（用户确认，2026-10-11），她不在或额度受阻时怎样处理待定 | manager-actions.md §6；data-model.md §5、§9.1 的 `notification_intent`；frontend.md §1 |

### 用量与额度

| 事项 | 设计与来源 |
|---|---|
| 读取上下文压缩：Claude 的 `system/compact_boundary`；Codex rollout 文件中的 `compacted`，只用于显示 | harness-adapter.md §1.3 第 4 条 |
| 每轮用量：Claude 的 `result.usage`；Codex 的 `turn.completed.usage` 是线程累计值，与上一轮做差 | harness-adapter.md §1.3 第 5 条 |
| 显示额度余量：Claude `rate_limit_event` 中的利用率与重置时间，Codex rollout 文件中的 `rate_limits`；额度域记下最近观测及其时间 | harness-adapter.md §1.3 第 5 条；data-model.md §8.2、§9.1；ui-ux-plan.md §8 第 2 条 |
| 额度检查的超时可配置，现在固定 2 分钟 | data-model.md §8.4 |

### harness 与执行现场

| 事项 | 设计与来源 |
|---|---|
| 每次启动记录 CLI 版本；CLI 升级后新出现的工具在 Inspector 中标出 | harness-adapter.md §1.5、§1.6；data-model.md §10.2 |
| "撤销这一轮"：用户命令，把槽位改写为上一个 turn 的采集 | harness-adapter.md §4.1；frontend.md §2 |
| 在官方应用中接管与交还：在途 turn 怎么处理，怎样确认外部使用已经结束，交还后核对 session、HEAD 与工作区 | [#1](https://github.com/vorton-lang/Lobotomy/issues/1)；manager-actions.md §4.3；frontend.md §2 |

### 审阅与试用

| 事项 | 设计与来源 |
|---|---|
| 审阅：diff 上的行内评论一次退回给执行者；改动可以看"这一轮相对上一轮" | ui-ux-plan.md §6、§8 第 3 条（用户确认，2026-10-12） |
| 在编辑器中打开候选成果 | ui-ux-plan.md §6、§8 第 5 条；本文"接触成果是一等入口"（用户确认，2026-10-12） |
| 试用的改进：阶段进展也能试用，不只是 done 的候选成果；在 GUI 或远程入口中看到试用的输出；Linux 上支持更多终端，并简化进程组接管（现在约 150 行握手，防的是罕见情况），需要在 Linux 桌面上实测，并补上 Linux 终端路径的测试 | #20；frontend.md §8 |

### 界面

| 事项 | 设计与来源 |
|---|---|
| Inspector：状态（含上下文用量）、输出、改动、操作；Angela 的"推测" | frontend.md §2；manager-actions.md §3.3 |
| 撤回或修改排队中的消息 | ui-ux-plan.md §6、§8 第 9 条（用户确认，2026-10-12） |
| `ask_user` 问题上的选项按钮 | frontend.md §2 |
| 按 agent 节流推送：现在后端每 100ms 查看所有事件日志，变化推给所有窗口 | frontend.md §3 第 3 条、§7.14 |
| 渲染：流式 Markdown 按块解析，封存已结束的块；Shiki 的 LRU 缓存；查看器中的 ANSI 颜色转为装饰 | frontend.md §4.1、§7.14 |

### 数据与存储

| 事项 | 设计与来源 |
|---|---|
| 大工具输出与保留的原始输出：所属任务关闭 30 天后回收全文；文件库的稀疏行索引 | data-model.md §7.3、§7.5 |
| 归属与物化记录补全：隔离的旧现场与试用副本现在只按目录确定，试用与验证现场只登记在后端内存中；遗留内容在项目打开时删除 | data-model.md §5、§6；frontend.md §8 |
| 清扫补全：没有记录的文件库文件、后端崩溃留下的 MCP 配置文件（含 token）、回收记录；删除失败后定期重试，GUI 显示占用的空间和原因 | data-model.md §6 |
| jj 中采集记录与 pin 的保留期 | data-model.md §6；[#6](https://github.com/vorton-lang/Lobotomy/issues/6) |

### 性能基线

| 事项 | 设计与来源 |
|---|---|
| 补上场景：多个 agent 同时流式输出、项目切换 | perf-baseline.md §5 |
| 性能预算：按基线结果确定 | frontend.md §5；#6 |

### 对外与远程

| 事项 | 设计与来源 |
|---|---|
| GitHub 接入：后端经 `gh` 读取并轮询项目状态，访问记入审计；写操作经 `request_external_action` 由用户确认；push 与成果导出 | manager-actions.md §5；#5 |
| Android 远程入口：后端要能从网络访问，需单独做安全设计 | [#23](https://github.com/vorton-lang/Lobotomy/issues/23) |

### Linux 适配

| 事项 | 设计与来源 |
|---|---|
| `PR_SET_PDEATHSIG` 已在代码中设置，在 Linux 上实测它的设置竞态与启动线程；后端重启后按命令行中的 turn 标识对账；在 Linux 上重跑 CLI 实测；CI 的 Linux 作业从 ubuntu-24.04 迁到 Ubuntu 26 | harness-adapter.md §1.8、§6；data-model.md §3.3；#6 |
