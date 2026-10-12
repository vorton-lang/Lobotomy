# Lobotomy

> 状态（2026-10-11）：M1 执行闭环、M2（第二个 harness：Claude Code）与 M3 多项目已经完成。M4 最小的全局 Manager Angela 的三步都已实现，等待真实使用中的验收，见 [Roadmap](notes/roadmap.md)。

一个长期在线、位于现有 coding harness 之上的 agent 组织运行时：用户主要与 Manager 协作，系统承载长期角色、原生 session、可恢复状态和组织级工作界面。

**Claude Code 接入是关键要求**，不能仅用 Claude 模型 API 或提示词替代。Dots 是设计参考，用于借鉴好的设计、避免问题；不是替代或差异化目标。

## 现在能做什么

现在有一个执行者 Malkuth。它默认使用官方 Codex CLI，也可以在设置中改用 Claude Code。在 GUI 中，你可以：

- 给 Malkuth 建任务，看它的对话、工具调用和汇报；
- 回答它提出的问题；它停下时，选择继续或新建会话；
- 看候选成果的交付说明、diff 和检查结果，然后验收或退回。验收后，成果发布到你仓库的分支上；
- 执行者提供试用命令时，点击后在电脑的独立终端里体验本轮候选副本。新一轮不会改写已经打开的旧副本。
- 同时管理多个项目：在左侧的项目列表中新建、切换、归档。
- 在「全部」中与 Angela 对话。她能读所有项目的现状，替你派活、验收、退回、放弃和暂停任务；你暂停的任务只有你能恢复。她回复中引用的原话和成果由运行时显示，点开成果是任务面板。
- 项目停下来等你、或者活干完了时，Angela 会自己醒来，写一条合并的消息。她要你决定的问题列在「等你决定」中。
- 你可以"回复"她的某一条消息，指定你在回什么；她转达你的话时会带上它，你的消息下写明转给了谁。
- 右侧栏的"共识"列出她记下的目标、决定和约束，各附你的原话。

审查还没有实现。

## 从源码运行

需要 Rust、Node.js、git，以及已经登录的 Codex CLI 或 Claude Code。

1. `cd frontend && npm install`（第一次，或依赖变了之后）
2. `npm start`：编译后端、构建前端，然后打开应用

应用发现后端已经在运行时，会连上它，不会启动新编译的那个。更新代码后，先在托盘中退出 Lobotomy，再运行 `npm start`。

第一次启动时，选择一个 git 仓库。仓库的工作区要干净，并且检出在分支上。开发方式见 [frontend.md](notes/frontend.md) §7；改什么跑什么测试，见 [AGENTS.md](AGENTS.md)。

## 资料

设计（`notes/`）：

- [Roadmap](notes/roadmap.md)：M1 执行闭环 → M2 第二个 harness → M3 多项目 → M4 最小的全局 Angela；其余按真实使用排序
- [Architecture](notes/architecture.md)：进程、代码布局、技术选择与测试
- [Data model](notes/data-model.md)：命令、占用、turn 与消息、任务流、副作用、对话记录、额度、项目与 host
- [Harness adapter](notes/harness-adapter.md)：CLI 调用方式、执行现场、成果采集与集成（含实测）
- [Roles and tasks](notes/roles-and-tasks.md)：角色类型与具名角色、任务生命周期、Workboard
- [Manager actions](notes/manager-actions.md)：向下转发、向上附件、记录、会议与对外操作
- Manager 的现行指令：[prompts/manager.md](backend/crates/lobotomyd/src/prompts/manager.md)（意图判断、信息过滤、任务翻译）
- [Frontend](notes/frontend.md)：Electron 形态、布局、前后端协议、渲染选型与现在的实现
- [UI/UX plan](notes/ui-ux-plan.md)：界面改进专项的原则、视觉方向与分阶段计划
- [Performance baseline](notes/perf-baseline.md)：GUI 性能基线的测法与参考数据
- [Ideas](notes/ideas.md)：暂不实现的小功能
- 技术路线：[#5](https://github.com/vorton-lang/Lobotomy/issues/5) 存储与权威来源的权衡，[#6](https://github.com/vorton-lang/Lobotomy/issues/6) 推荐实现（SQLite + jj + Rust 后端 + Electron 前端）

调研与背景（`notes/research/`）：

- [harness 接入与条款](notes/research/harness-interfaces.md)、[agent GUI 前端](notes/research/frontend-survey.md)；实测脚本见 `spikes/`
- [UI/UX survey](notes/research/ui-ux-survey.md)：多 agent 工具的界面与交互、同类产品用户的痛点（2026-10-11/12 快照）
- [Organization Runtime](notes/research/organization-runtime.md)：早期架构笔记（角色、session、持久状态和 GUI）
- [HCI rationale](notes/research/hci-rationale.md)：人机协作研究节选
- [Market landscape](notes/research/market-landscape.md)：2026-10-01 调研草稿，产品事实与市场判断待核实

## 许可证

本仓库采用 [MIT 许可证](LICENSE)。
