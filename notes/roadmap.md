# Roadmap

> Status: 规划 v0.3（2026-10-10）
> v0.3 调整顺序（**语义更新**，用户确认，2026-10-10）：M3 改为多项目，最小的全局 Angela 顺延为 M4。依据：Angela 在项目外层，她的数据位置、工具参数和界面都取决于多项目的结构；先做 Angela 会产生临时方案（[data-model.md](data-model.md) §10）。
> v0.2（2026-10-09）：M1 之后先接入第二个 harness，再做最小的全局 Angela。依据：[#23](https://github.com/vorton-lang/Lobotomy/issues/23) 的 Manager 定位，以及 [#20](https://github.com/vorton-lang/Lobotomy/issues/20)–[#22](https://github.com/vorton-lang/Lobotomy/issues/22) 的讨论。
> v0.1 的编号：M2 审查、M3 Binah 与会议（含 Claude adapter）、M4 Angela。旧 issue 中的编号按此对应。

## 原则

- **每个里程碑都端到端可用。** 先打通一条完整链路，再增加对象种类和调度能力。
- **主路径先存在。** 主路径是用户经 Angela 管理项目，借鉴 OpenAI Dots。用户也可以直接打开 Malkuth 等角色的窗口。这条直接通路是常设的二等入口，不是临时方案；用户自然选择，不强制。M1 已经验证"没有 Angela，控制流也能跑通"。v0.1 把 Angela 放在最后；现在先做她所依赖的多项目结构，再做最小的 Angela，其余按真实使用排序。
- **接触成果是一等入口，直接指挥执行者是二等入口。** 用户在任何地方都能直接查看和试用成果。Angela 对话中引用的成果，点开就是任务面板中同一个 diff 与试用视图。二等入口指的是另一件事：绕过 Angela，直接给执行者发消息或建任务。依据：[#20](https://github.com/vorton-lang/Lobotomy/issues/20) 约束 5 要求用户能直接接触成果；Dots 式的主路径约束的是指挥执行者，不是查看成果（用户确认，2026-10-11）。
- **事实、权威、判断分开。** 运行时持有事实，用户持有权威，Angela 与各 role 给出判断。判断不能冒充事实，也不能冒充权威。现有的例子：原文由运行时插入，Manager 只引用 ID；`decision` 必须引用用户原话；只有状态能触发通知；记忆不能给模型扩权（[#22](https://github.com/vorton-lang/Lobotomy/issues/22) §4）。新设计按这一条检查：事实归谁，权威归谁，判断有没有机会伪装成另外两者（用户确认，2026-10-11）。
- **角色不绑定 harness。** 每个 role 都能换 harness，Angela 也一样。外部测试者没有 Claude，所有 role 都要能在 Codex 上运行。官方 CLI 担任 Angela 表现不佳时，可以改用自建的 API harness（用户确认，2026-10-09）。
- **长期记忆由 Lobotomy 保存。** 不依赖 harness 自带的记忆或原生会话的上下文：harness 和模型会换，它们的上下文窗口和注意力也不同（用户确认，2026-10-09）。
- **性能基线从 M1 开始**（[frontend.md](frontend.md) §5）。
- 在 Windows 上开发，不依赖平台特有行为；正式发版前集中修 Linux 问题。

## 里程碑

| 里程碑 | 内容 |
|---|---|
| **M1 执行闭环**（完成） | 用户在 GUI 中给 Malkuth 建任务 → 执行 → 采集并固定成果 → 验证 → 用户验收 → 发布 → 物化预览。Rust 后端、SQLite 命令入口、Codex adapter、jj 采集与发布、Electron 界面、性能基线框架。用户与测试者在 Windows 和 Linux 上用真实 Codex 走通过全链路 |
| **M2 第二个 harness**（完成，2026-10-10） | Claude adapter，见下 |
| **M3 多项目**（完成，2026-10-11） | 见下 |
| **M4 最小的全局 Angela** | 见下 |

### M2 第二个 harness（完成）

- Claude adapter 按 [harness-adapter.md](harness-adapter.md) §1.2–1.4 的实测结果实现：stream-json 事件解析为 item；session ID 由运行时指定；每个 turn 重传角色指令；用量与 `rate_limit_event`；两种权限模式；原生中断。
- 真实 Claude（haiku）走完建任务、done、采集、验证到等待验收；中断后继续，resume 同一个会话。都写成了默认不运行的后端测试（harness-adapter.md §2）。
- 假 Claude 的后端测试覆盖与 Claude 有关的路径：完成到验收、续用会话、额度被拒只阻塞 Claude 的额度域、中断。采集、验证、验收等其余场景走同一套运行时代码，没有逐个复制一遍。
- 适配器接口仍按 `Harness` 枚举匹配，没有抽象成 trait。自建的 API harness 可以做成同样形状的可执行程序（harness-adapter.md §1.3 第 7 条）。
- 设置中可以切换每个角色的 harness；MCP 契约测试与性能基线已补上。
- 没有验证：自动审批模式在受管环境中的表现（本机没有受管环境）；权限模式被拒时 Claude 的报错文字；`-p` 下额度被拒的实际形式；Linux 上的 Claude。

### M3 多项目（完成）

2026-10-11 用户用两个真实仓库走完验收。验收中发现的设置页白屏已修复（[#51](https://github.com/vorton-lang/Lobotomy/pull/51)）。

设计见 [data-model.md](data-model.md) §10、[frontend.md](frontend.md) §1–§3、[harness-adapter.md](harness-adapter.md) §2。

- 一个后端进程管理所有项目。项目登记在 host 中；各项目的数据目录、命令与规则不变。
- GUI：项目列表、切换项目、在 GUI 中新建项目（不重启后端）、全局汇总的"等你决定"。
- 归档与取消归档；打不开的项目只影响自己。
- 旧的单项目数据升级后照常使用。
- 设计中定下 Angela 在 host 的位置（data-model.md §10.6），M4 实现。
- 实现分 3 步，每步做完都能使用：
  1. 后端：host 中的项目登记；一个后端进程打开所有项目，打不开的项目只影响自己；MCP 地址与 GUI 协议带项目 ID；Electron 只启动一个后端；旧数据迁移。界面仍只显示一个项目。（完成，2026-10-11）
  2. GUI：项目列表、切换项目、新建项目、全局汇总的"等你决定"。
  3. 归档、取消归档、重试打开。
- 验收：
  - 同时管理两个真实仓库，各自走完 M1 的全链路，互不干扰；
  - 一个项目的 Claude 额度被拒时，所有项目中使用 Claude 的 role 都停下，使用 Codex 的照常运行；
  - 在 GUI 中新建项目，不重启后端；归档后这个项目不再调度，取消归档后继续；
  - 一个项目的数据目录丢失时，其他项目照常运行；
  - 现有的单项目数据升级后照常使用。

### M4 最小的全局 Angela

**第一优先级是降低用户回来接上工作的成本**（[#50](https://github.com/vorton-lang/Lobotomy/issues/50)，用户确认，2026-10-11）。用户回来时，应能直接试用成果、看懂关键变化、表达下一步意见，不必从聊天记录重新讲起。

- 全局只有一个 Angela，放在 host（data-model.md §10.6）。她能看到所有未归档的项目，工具都带项目 ID。
- 主区是与 Angela 的对话。各项目中 Malkuth 的对话、任务面板和"等你决定"保留为直接入口。Angela 引用的成果，点开就是任务面板中同一个 diff 与试用视图（见"原则"）。
- 工具就是 GUI 已有的命令（建任务、发消息、读任务与成果、`ask_user`），加上原话转发与附件引用（[manager-actions.md](manager-actions.md) §1、§2）。派活的 `assign` 必须写明允许的动作和停止条件（[roles-and-tasks.md](roles-and-tasks.md) §4）。
- Angela 的指令（Manager 的思维方式、决定下一步的程序、派活与上行过滤的规则、难例）先用 Ember 中现在的[草稿](https://github.com/vorton-lang/Ember/blob/main/manager-interviews/manager-draft.md)，再按真实使用中的失败迭代。这些失败也作为面试的新案例（hold-out）。
  - **语义更新**（用户确认，2026-10-11）：原来写的是指令"在 Ember 中按面试迭代，成熟后作为 Lobotomy 的角色指令"。
  - 依据：#50 的检验标准是用户少重述、少追问、少返工，成果被实际使用，改方向的成本低。这些只能在自然协作中观察，面试的静态案例测不出来。第三轮面试中，结构约束守住了，指令中的语义规则没有守住（[Ember 第三轮](https://github.com/vorton-lang/Ember/tree/main/manager-interviews/2026-10-11-official-cli-round3)）。所以 #50 中能做成结构的原则做进运行时，见下面关于暂停、读取边界和附件的三条。
- 先不定 Angela 用哪个模型（用户判断，2026-10-11）。依据：两轮面试中，候选之间没有本质差距，起作用的主要是指令、环境、记忆和约束；这些搭完整后，不同模型的操作空间会收束。以后再找默认取向正好对齐这个岗位的模型。
- 记录（ledger）只做最小的一版：目标、决定、约束带用户原话作依据，推测单独存放；开新会话时投影进首轮输入，也可经 MCP 查询（manager-actions.md §3）。
- 用户直接操作项目后，Angela 从事件中得知变化，不要求用户重述，也不重复旧操作（#23 §3）。用户设的暂停只有用户能解除（roles-and-tasks.md §4）。
- Angela 的读取结果写明边界：每个项目读到哪里为止，哪些项目没有读到，每条结果是否仍然有效（data-model.md §10.6）。
- 附件写明它绑定的版本，运行时按当前状态标出它是否已过期（manager-actions.md §2.1）。
- 收到模糊反馈时，Angela 不把理解的负担推回给用户，先在已授权范围内调查或做小实验，再带结果回来（#21）。这是对她行为的检查项，不是要统计的指标。
- 实现分 3 步，每步做完都能使用（2026-10-11）：
  1. **Angela 能对话、能读。** 她的数据库与 turn 在 host 目录下；「全部」的主区是与她的对话，所有项目的"等你决定"移到右侧栏；她经 MCP 读所有项目，读取结果带边界。（完成，2026-10-11）
     - 她的指令改自 Ember 的草稿：去掉了还没有的机制（`[[quote:…]]`、派活），写明她现在只能读，需要建任务时把任务写好交给用户。
     - 设置中可以切换她的 harness。没有用真实的 CLI 走过她的 turn。
     - 没做：输入中没有用户的本地时间。草稿的"时间与节奏"依赖它，现在只保留了按用户原话收尾的规则。
  2. **Angela 能操作。** 调用者 `manager`；派活、转发原话、验收、退回、放弃、暂停，其中用户设的暂停只有用户能解除；回复中的 `[[quote:…]]` 与 `[[att:…]]` 由运行时渲染，附件带版本与当前有效性，点开是任务面板。（完成，2026-10-11）
     - 她的工具：`assign`、`send_to_role`、`accept`、`send_back`、`abandon`、`set_paused`。命令的划分见 data-model.md §10.6 跨库操作第 2 条。
     - 她的输入中，用户的消息带消息 ID，供 `quote` 和 `[[quote:…]]` 引用。
     - 任务面板显示她派活时写的解读、允许的动作和停止条件，并写明任务是谁暂停的。
     - 没做，按依赖排到后面（用户确认，2026-10-11）：
       - 第 3 步：`send_to_role` 的 `ledger` 参数，因为记录在第 3 步才有；`flag_mismatch`，它要用第 3 步把项目事件送进 Angela 收件箱的通道，现在执行者的指令要求它在汇报中指出原话与解读的冲突；用户消息下"已转给 …"的标签。
       - GitHub 接入之后：`assign` 的 `links`。
       - 第 3 步：`send_to_role` 的 `context`，以及 GUI 中用户手动指定回复对象，设计见 manager-actions.md §1.1（用户确认，2026-10-11）。
     - 只读调查的任务产生了文件改动时，运行时标出（roles-and-tasks.md §4）。
     - 实测（2026-10-11，Windows，Claude Code，haiku）：她先读项目，再调用 `assign`，`actions`、`quote`、`stop_when` 都按工具的参数说明填写，引用的是用户那条消息的 ID。测试是 `a_real_claude_angela_assigns_a_task`，默认忽略。只验证了一次调用，没有验证她在自然协作中的判断。
  3. **Angela 跟上变化。** 运行时按每个项目的事件游标把变化放进她的收件箱；组织进入"等你"时她写合并的消息；`ask_user`；最小的记录与换会话时的投影。还有第 2 步排到这里的几项：`ledger` 参数、`flag_mismatch`、"已转给 …"标签、`context` 与用户手动指定回复对象。（完成，2026-10-11）
     - 没做：她的输入中仍然没有用户的本地时间（第 1 步留下的）；`ask_user` 和 `flag_mismatch` 不带附件；下层对记录的只读查询。系统通知还没有做；做的时候，等她写完合并的消息再发（manager-actions.md §6，用户确认，2026-10-11）。
     - 这一步的行为只用假 CLI 测过。用真实的 CLI 走过的只有第 2 步的 `assign`。
     - 已完成：收件箱与唤醒。项目停下且有她不知道的变化时唤醒她，她每次运行时收到各项目的变化（manager-actions.md §6"Manager 什么时候运行"）。
     - 已完成：`ask_user`，只带问题；要停下的任务由她另外暂停（manager-actions.md §6 现状）。
     - 已完成：最小的记录。按作用域存放，换会话时放进她的首轮输入，「全部」中有只读的"共识"（manager-actions.md §3.4.1）。
     - 已完成：`flag_mismatch`，直接进入用户与 Angela 的对话，并唤醒她（manager-actions.md §2.3 现状）。
     - 已完成：`send_to_role` 与 `assign` 的 `ledger` 参数（manager-actions.md §3.4.1）。
     - 已完成：用户在 GUI 中"回复"Angela 的某一条；`context` 与 `recent_replies`；用户消息下的"已转给 …"（manager-actions.md §1.1 现状）。
- #50 的三组开放问题属于 Angela 的判断：怎样估计用户的状态，怎样选择行动与介入时机，给出什么信息。M4 不回答这些问题，只搭出能观察它们的环境。M4 不做用户状态模型、信息价值的打分、为对照而做的多个版本，也不改通知规则。需要提前告诉用户的变化，Angela 按现有规则改变状态，例如用 `ask_user` 阻塞任务，由运行时发通知（manager-actions.md §6）。
- 验收：
  - 用户只与 Angela 对话，完成一个真实任务；
  - 用户离开一段时间后回来，Angela 的消息给出可运行的入口、关键变化、同一版本的验证结果和真正待决的分叉，用户不重述就能给出下一步意见。从自然协作中观察重述、追问和返工，不加评价表（#50）；
  - 用户直接暂停任务或修改完成条件后，Angela 按新状态继续；
  - 查询有缺口时，Angela 讲清缺口：不把没有读到说成没有，也不把过期的结果当作当前事实；
  - Angela 改用 Codex 后仍然可用。

M4 之后，用真实项目使用 Lobotomy，包括开发它自己，再按实际痛点从下面的待排事项中排序。

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

| 事项 | 来源 |
|---|---|
| 界面与交互的改进专项：视觉基础、角色与状态、对话、"等你决定"、任务面板、窗口与导航。只改前端，与 M4 的验收并行；计划是草案，待用户确认 | [ui-ux-plan.md](ui-ux-plan.md)（2026-10-11） |
| 审查：Yesod、独立审查现场、审查结论；一轮返工预算与结论失效规则 | v0.1 的 M2；[#19](https://github.com/vorton-lang/Lobotomy/issues/19) |
| 试用的改进：阶段进展也能试用，不只是 done 的候选成果；在 GUI 或远程入口中看到试用的输出；Linux 上支持更多终端，并简化进程组接管（现在约 150 行握手，防的是罕见情况），需要在 Linux 桌面上实测，并补上 Linux 终端路径的测试 | #20；frontend.md §8 |
| 长期记忆扩展：版本化修订、作用域、纠正后通知受影响的任务 | #22 |
| Binah 与会议；研究任务按非代码证据完成 | v0.1 的 M3；[#5](https://github.com/vorton-lang/Lobotomy/issues/5) |
| 对外操作与导出：GitHub 写操作的确认、push、成果导出 | #5；manager-actions.md §5 |
| Android 远程入口：要让后端可以从网络访问，需单独做安全设计 | #23 |
| 在官方应用中接管与交还：在途 turn 怎么处理，怎样确认外部使用已经结束，交还后核对 session、HEAD 与工作区 | [#1](https://github.com/vorton-lang/Lobotomy/issues/1) |
| Linux 适配：`PR_SET_PDEATHSIG`；后端重启后按命令行中的 turn 标识对账；在 Linux 上重跑实测；CI 的 Linux 作业从 ubuntu-24.04 迁到 Ubuntu 26；Linux CI 的偶发超时 | [#6](https://github.com/vorton-lang/Lobotomy/issues/6)；[#25](https://github.com/vorton-lang/Lobotomy/issues/25)；harness-adapter.md §1.8 |
| jj 中采集记录的保留期；性能预算 | #6；frontend.md §5 |
