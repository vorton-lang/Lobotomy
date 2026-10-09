# Lobotomy

> 状态（2026-10-09）：M1 执行闭环已经可以运行。M2 及以后的内容（审查、Binah 与会议、Manager Angela）仍是规划草稿。

一个长期在线、位于现有 coding harness 之上的 agent 组织运行时：用户主要与 Manager 协作，系统承载长期角色、原生 session、可恢复状态和组织级工作界面。

**Claude Code 接入是关键要求**，不能仅用 Claude 模型 API 或提示词替代。Dots 是设计参考，用于借鉴好的设计、避免问题；不是替代或差异化目标。

## 现在能做什么（M1）

M1 有一个执行者 Malkuth，它使用官方 Codex CLI。在 GUI 中，你可以：

- 给 Malkuth 建任务，看它的对话、工具调用和汇报；
- 回答它提出的问题；它停下时，选择继续或新建会话；
- 看候选成果的 diff 和检查结果，然后验收或退回。验收后，成果发布到你仓库的分支上。

Claude Code 接入、审查和 Manager 还没有实现，见 [Roadmap](notes/roadmap.md)。

## 从源码运行

需要 Rust、Node.js、git，以及已经登录的 Codex CLI。

1. `cd backend && cargo build -p lobotomyd`
2. `cd frontend && npm install && npm run app`

第一次启动时，选择一个 git 仓库。仓库的工作区要干净，并且检出在分支上。开发和测试方式见 [frontend.md](notes/frontend.md) §7。

## 资料

- [Manager JD](notes/manager-jd.md)：意图判断、信息过滤、任务翻译与必要时的直接对齐
- [Organization Runtime](notes/organization-runtime.md)：角色、session、持久状态和 GUI
- [Harness adapter](notes/harness-adapter.md)：CLI 调用方式、执行现场、成果采集与集成（含实测）
- 技术路线：[#5](https://github.com/vorton-lang/Lobotomy/issues/5) 存储与权威来源的权衡，[#6](https://github.com/vorton-lang/Lobotomy/issues/6) 推荐实现（SQLite + jj + Rust 后端 + Electron 前端）
- [Manager actions](notes/manager-actions.md)：向下转发、向上附件、记录、会议与对外操作
- [Roles and tasks](notes/roles-and-tasks.md)：角色类型与具名角色、任务生命周期、Workboard
- [Frontend](notes/frontend.md)：Electron 形态、布局、前后端协议与渲染选型
- [Roadmap](notes/roadmap.md)：M1 执行闭环 → M2 审查 → M3 Binah 与会议 → M4 Angela
- [Ideas](notes/ideas.md)：暂不实现的小功能
- 调研快照：[harness 接入与条款](notes/research/harness-interfaces.md)、[agent GUI 前端](notes/research/frontend-survey.md)；实测脚本见 `spikes/`
- [HCI rationale](notes/hci-rationale.md)：人机协作研究节选
- [Market landscape](notes/market-landscape.md)：2026-10-01 调研草稿，产品事实与市场判断待核实

## 许可证

本仓库采用 [MIT 许可证](LICENSE)。
