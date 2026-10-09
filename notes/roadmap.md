# Roadmap

> Status: 规划 v0.2（2026-10-09）
> v0.2 调整顺序（**语义更新**，用户确认，2026-10-09）：M1 之后先接入第二个 harness，再做最小的全局 Angela。依据：[#23](https://github.com/vorton-lang/Lobotomy/issues/23) 的 Manager 定位，以及 [#20](https://github.com/vorton-lang/Lobotomy/issues/20)–[#22](https://github.com/vorton-lang/Lobotomy/issues/22) 的讨论。
> v0.1 的编号：M2 审查、M3 Binah 与会议（含 Claude adapter）、M4 Angela。旧 issue 中的编号按此对应。

## 原则

- **每个里程碑都端到端可用。** 先打通一条完整链路，再增加对象种类和调度能力。
- **主路径先存在。** 主路径是用户经 Angela 管理项目，借鉴 OpenAI Dots。用户也可以直接打开 Malkuth 等角色的窗口。这条直接通路是常设的二等入口，不是临时方案；用户自然选择，不强制。M1 已经验证"没有 Angela，控制流也能跑通"。v0.1 把 Angela 放在最后；现在改为先做最小的 Angela，其余按真实使用排序。
- **角色不绑定 harness。** 每个 role 都能换 harness，Angela 也一样。外部测试者没有 Claude，所有 role 都要能在 Codex 上运行。官方 CLI 担任 Angela 表现不佳时，可以改用自建的 API harness（用户确认，2026-10-09）。
- **长期记忆由 Lobotomy 保存。** 不依赖 harness 自带的记忆或原生会话的上下文：harness 和模型会换，它们的上下文窗口和注意力也不同（用户确认，2026-10-09）。
- **性能基线从 M1 开始**（[frontend.md](frontend.md) §5）。
- 在 Windows 上开发，不依赖平台特有行为；正式发版前集中修 Linux 问题。

## 里程碑

| 里程碑 | 内容 |
|---|---|
| **M1 执行闭环**（完成） | 用户在 GUI 中给 Malkuth 建任务 → 执行 → 采集并固定成果 → 验证 → 用户验收 → 发布 → 物化预览。Rust 后端、SQLite 命令入口、Codex adapter、jj 采集与发布、Electron 界面、性能基线框架 |
| **M2 第二个 harness** | Claude adapter，见下 |
| **M3 最小的全局 Angela** | 见下 |

### M2 第二个 harness

- Claude adapter，按 [harness-adapter.md](harness-adapter.md) §1.2–1.4 的实测结果实现：stream-json 事件解析为 item；session ID 由运行时指定；每个 turn 重传角色指令（压缩后必需）；用量与 `rate_limit_event`；两种权限模式；原生中断。
- 有了两个真实 harness 后，定下适配器接口。接口不假定 harness 有原生 resume，以后可以接入会话由 Lobotomy 保存的自建 API harness（[organization-runtime.md](organization-runtime.md) §7）。
- 每个 role 使用哪个 harness，由配置决定。没有安装 Claude 时，一切照常运行。
- 验收：
  - Malkuth 改用 Claude，走完 M1 的全链路；
  - 后端测试用假 Claude CLI 覆盖与 Codex 相同的场景；
  - MCP 契约测试、性能基线补上 Claude 的流式文本。

### M3 最小的全局 Angela

- 全局只有一个 Angela，放在项目外层（[data-model.md](data-model.md) §10）。第一版只连当前一个项目。
- 主区是与 Angela 的对话。Malkuth 的对话、任务面板和"等你决定"保留为直接入口。
- 工具就是 GUI 已有的命令（建任务、发消息、读任务与成果、`ask_user`），加上原话转发与附件引用（[manager-actions.md](manager-actions.md) §1、§2）。
- 记录（ledger）只做最小的一版：目标、决定、约束带用户原话作依据，推测单独存放；开新会话时投影进首轮输入，也可经 MCP 查询（manager-actions.md §3）。
- 用户直接操作项目后，Angela 从事件中得知变化，不要求用户重述，也不重复旧操作（#23 §3）。
- 收到模糊反馈时，Angela 不把理解的负担推回给用户，先在已授权范围内调查或做小实验，再带结果回来（#21）。这是对她行为的检查项，不是要统计的指标。
- 验收：
  - 用户只与 Angela 对话，完成一个真实任务；
  - 用户直接暂停任务或修改完成条件后，Angela 按新状态继续；
  - Angela 改用 Codex 后仍然可用。

M3 之后，用真实项目使用 Lobotomy，包括开发它自己，再按实际痛点从下面的待排事项中排序。

### #20 的约束在哪里检验

| 约束 | 检验 |
|---|---|
| 1 先提供低成本的真实体验 | M3 中 Angela 用已有的 diff 与报告；"验收前试用成果"待排 |
| 2 反馈影响后续自主选择 | M3：ledger 中的决定与约束进入后续任务说明 |
| 3 保留自主展开的空间 | M3：Angela 的行为检查 |
| 4 关键变化时重新共同判断 | M3：`ask_user` 只用于关键取舍 |
| 5 用户能直接接触成果 | M1 已有；M3 保留为常设入口 |
| 6 共识是可修订的工作依据 | M3：原话与推测分开，变更用 `supersedes` |

## 待排

| 事项 | 来源 |
|---|---|
| 审查：Yesod、独立审查现场、审查结论；一轮返工预算与结论失效规则 | v0.1 的 M2；[#19](https://github.com/vorton-lang/Lobotomy/issues/19) |
| 验收前试用成果：候选成果或阶段进展物化成可以运行的目录 | #20 |
| 多项目：一个后端进程开多个项目实例；Angela 跨项目工作 | #23；data-model.md §10 |
| 长期记忆扩展：版本化修订、作用域、纠正后通知受影响的任务 | #22 |
| Binah 与会议；研究任务按非代码证据完成 | v0.1 的 M3；[#5](https://github.com/vorton-lang/Lobotomy/issues/5) |
| 对外操作与导出：GitHub 写操作的确认、push、成果导出 | #5；manager-actions.md §5 |
| Android 远程入口：要让后端可以从网络访问，需单独做安全设计 | #23 |
| 在官方应用中接管与交还：在途 turn 怎么处理，怎样确认外部使用已经结束，交还后核对 session、HEAD 与工作区 | [#1](https://github.com/vorton-lang/Lobotomy/issues/1) |
| Linux 适配：`PR_SET_PDEATHSIG`；后端重启后按命令行中的 turn 标识对账；在 Linux 上重跑实测；CI 的 Linux 作业从 ubuntu-24.04 迁到 Ubuntu 26 | [#6](https://github.com/vorton-lang/Lobotomy/issues/6)；harness-adapter.md §1.8 |
| jj 中采集记录的保留期；性能预算 | #6；frontend.md §5 |
