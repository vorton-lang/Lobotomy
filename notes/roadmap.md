# Roadmap

> Status: 规划 v0.3（2026-10-10）
> v0.3 调整顺序（**语义更新**，用户确认，2026-10-10）：M3 改为多项目，最小的全局 Angela 顺延为 M4。依据：Angela 在项目外层，她的数据位置、工具参数和界面都取决于多项目的结构；先做 Angela 会产生临时方案（[data-model.md](data-model.md) §10）。
> v0.2（2026-10-09）：M1 之后先接入第二个 harness，再做最小的全局 Angela。依据：[#23](https://github.com/vorton-lang/Lobotomy/issues/23) 的 Manager 定位，以及 [#20](https://github.com/vorton-lang/Lobotomy/issues/20)–[#22](https://github.com/vorton-lang/Lobotomy/issues/22) 的讨论。
> v0.1 的编号：M2 审查、M3 Binah 与会议（含 Claude adapter）、M4 Angela。旧 issue 中的编号按此对应。

## 原则

- **每个里程碑都端到端可用。** 先打通一条完整链路，再增加对象种类和调度能力。
- **主路径先存在。** 主路径是用户经 Angela 管理项目，借鉴 OpenAI Dots。用户也可以直接打开 Malkuth 等角色的窗口。这条直接通路是常设的二等入口，不是临时方案；用户自然选择，不强制。M1 已经验证"没有 Angela，控制流也能跑通"。v0.1 把 Angela 放在最后；现在先做她所依赖的多项目结构，再做最小的 Angela，其余按真实使用排序。
- **角色不绑定 harness。** 每个 role 都能换 harness，Angela 也一样。外部测试者没有 Claude，所有 role 都要能在 Codex 上运行。官方 CLI 担任 Angela 表现不佳时，可以改用自建的 API harness（用户确认，2026-10-09）。
- **长期记忆由 Lobotomy 保存。** 不依赖 harness 自带的记忆或原生会话的上下文：harness 和模型会换，它们的上下文窗口和注意力也不同（用户确认，2026-10-09）。
- **性能基线从 M1 开始**（[frontend.md](frontend.md) §5）。
- 在 Windows 上开发，不依赖平台特有行为；正式发版前集中修 Linux 问题。

## 里程碑

| 里程碑 | 内容 |
|---|---|
| **M1 执行闭环**（完成） | 用户在 GUI 中给 Malkuth 建任务 → 执行 → 采集并固定成果 → 验证 → 用户验收 → 发布 → 物化预览。Rust 后端、SQLite 命令入口、Codex adapter、jj 采集与发布、Electron 界面、性能基线框架。用户与测试者在 Windows 和 Linux 上用真实 Codex 走通过全链路 |
| **M2 第二个 harness**（完成，2026-10-10） | Claude adapter，见下 |
| **M3 多项目** | 见下 |
| **M4 最小的全局 Angela** | 见下 |

### M2 第二个 harness（完成）

- Claude adapter 按 [harness-adapter.md](harness-adapter.md) §1.2–1.4 的实测结果实现：stream-json 事件解析为 item；session ID 由运行时指定；每个 turn 重传角色指令；用量与 `rate_limit_event`；两种权限模式；原生中断。
- 真实 Claude（haiku）走完建任务、done、采集、验证到等待验收；中断后继续，resume 同一个会话。都写成了默认不运行的后端测试（harness-adapter.md §2）。
- 假 Claude 的后端测试覆盖与 Claude 有关的路径：完成到验收、续用会话、额度被拒只阻塞 Claude 的额度域、中断。采集、验证、验收等其余场景走同一套运行时代码，没有逐个复制一遍。
- 适配器接口仍按 `Harness` 枚举匹配，没有抽象成 trait。自建的 API harness 可以做成同样形状的可执行程序（harness-adapter.md §1.3 第 7 条）。
- 设置中可以切换每个角色的 harness；MCP 契约测试与性能基线已补上。
- 没有验证：自动审批模式在受管环境中的表现（本机没有受管环境）；权限模式被拒时 Claude 的报错文字；`-p` 下额度被拒的实际形式；Linux 上的 Claude。

### M3 多项目

设计见 [data-model.md](data-model.md) §10、[frontend.md](frontend.md) §1–§3、[harness-adapter.md](harness-adapter.md) §2。

- 一个后端进程管理所有项目。项目登记在 host 中；各项目的数据目录、命令与规则不变。
- GUI：项目列表、切换项目、在 GUI 中新建项目（不重启后端）、全局汇总的"等你决定"。
- 归档与取消归档；打不开的项目只影响自己。
- 旧的单项目数据升级后照常使用。
- 设计中定下 Angela 在 host 的位置（data-model.md §10.6），M4 实现。
- 验收：
  - 同时管理两个真实仓库，各自走完 M1 的全链路，互不干扰；
  - 一个项目的 Claude 额度被拒时，所有项目中使用 Claude 的 role 都停下，使用 Codex 的照常运行；
  - 在 GUI 中新建项目，不重启后端；归档后这个项目不再调度，取消归档后继续；
  - 一个项目的数据目录丢失时，其他项目照常运行；
  - 现有的单项目数据升级后照常使用。

### M4 最小的全局 Angela

- 全局只有一个 Angela，放在 host（data-model.md §10.6）。她能看到所有未归档的项目，工具都带项目 ID。
- 主区是与 Angela 的对话。各项目中 Malkuth 的对话、任务面板和"等你决定"保留为直接入口。
- 工具就是 GUI 已有的命令（建任务、发消息、读任务与成果、`ask_user`），加上原话转发与附件引用（[manager-actions.md](manager-actions.md) §1、§2）。
- 记录（ledger）只做最小的一版：目标、决定、约束带用户原话作依据，推测单独存放；开新会话时投影进首轮输入，也可经 MCP 查询（manager-actions.md §3）。
- 用户直接操作项目后，Angela 从事件中得知变化，不要求用户重述，也不重复旧操作（#23 §3）。
- 收到模糊反馈时，Angela 不把理解的负担推回给用户，先在已授权范围内调查或做小实验，再带结果回来（#21）。这是对她行为的检查项，不是要统计的指标。
- 验收：
  - 用户只与 Angela 对话，完成一个真实任务；
  - 用户直接暂停任务或修改完成条件后，Angela 按新状态继续；
  - Angela 改用 Codex 后仍然可用。

M4 之后，用真实项目使用 Lobotomy，包括开发它自己，再按实际痛点从下面的待排事项中排序。

### #20 的约束在哪里检验

| 约束 | 检验 |
|---|---|
| 1 先提供低成本的真实体验 | 已有：交付说明与候选成果试用（frontend.md §8）。M4 中 Angela 用已有的 diff、报告与试用；试用的改进待排 |
| 2 反馈影响后续自主选择 | M4：ledger 中的决定与约束进入后续任务说明 |
| 3 保留自主展开的空间 | M4：Angela 的行为检查 |
| 4 关键变化时重新共同判断 | M4：`ask_user` 只用于关键取舍 |
| 5 用户能直接接触成果 | M1 已有；M4 保留为常设入口 |
| 6 共识是可修订的工作依据 | M4：原话与推测分开，变更用 `supersedes` |

## 待排

| 事项 | 来源 |
|---|---|
| 审查：Yesod、独立审查现场、审查结论；一轮返工预算与结论失效规则 | v0.1 的 M2；[#19](https://github.com/vorton-lang/Lobotomy/issues/19) |
| 试用的改进：阶段进展也能试用，不只是 done 的候选成果；在 GUI 或远程入口中看到试用的输出；Linux 上支持更多终端，并简化进程组接管（现在约 150 行握手，防的是罕见情况），需要在 Linux 桌面上实测，并补上 Linux 终端路径的测试 | #20；frontend.md §8 |
| 长期记忆扩展：版本化修订、作用域、纠正后通知受影响的任务 | #22 |
| Binah 与会议；研究任务按非代码证据完成 | v0.1 的 M3；[#5](https://github.com/vorton-lang/Lobotomy/issues/5) |
| 对外操作与导出：GitHub 写操作的确认、push、成果导出 | #5；manager-actions.md §5 |
| Android 远程入口：要让后端可以从网络访问，需单独做安全设计 | #23 |
| 在官方应用中接管与交还：在途 turn 怎么处理，怎样确认外部使用已经结束，交还后核对 session、HEAD 与工作区 | [#1](https://github.com/vorton-lang/Lobotomy/issues/1) |
| Linux 适配：`PR_SET_PDEATHSIG`；后端重启后按命令行中的 turn 标识对账；在 Linux 上重跑实测；CI 的 Linux 作业从 ubuntu-24.04 迁到 Ubuntu 26；Linux CI 的偶发超时 | [#6](https://github.com/vorton-lang/Lobotomy/issues/6)；[#25](https://github.com/vorton-lang/Lobotomy/issues/25)；harness-adapter.md §1.8 |
| jj 中采集记录的保留期；性能预算 | #6；frontend.md §5 |
