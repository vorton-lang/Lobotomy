# Lobotomy

> 状态（2026-10-10）：M1 执行闭环与 M2（第二个 harness：Claude Code）已经完成。下一步是 M3 多项目，然后是 M4 最小的全局 Manager Angela，见 [Roadmap](notes/roadmap.md)。

一个长期在线、位于现有 coding harness 之上的 agent 组织运行时：用户主要与 Manager 协作，系统承载长期角色、原生 session、可恢复状态和组织级工作界面。

**Claude Code 接入是关键要求**，不能仅用 Claude 模型 API 或提示词替代。Dots 是设计参考，用于借鉴好的设计、避免问题；不是替代或差异化目标。

## 现在能做什么

现在有一个执行者 Malkuth。它默认使用官方 Codex CLI，也可以在设置中改用 Claude Code。在 GUI 中，你可以：

- 给 Malkuth 建任务，看它的对话、工具调用和汇报；
- 回答它提出的问题；它停下时，选择继续或新建会话；
- 看候选成果的 diff 和检查结果，然后验收或退回。验收后，成果发布到你仓库的分支上。

多项目、审查和 Manager 还没有实现。

## 从源码运行

需要 Rust、Node.js、git，以及已经登录的 Codex CLI 或 Claude Code。

1. `cd backend && cargo build -p lobotomyd`
2. `cd frontend && npm install && npm run app`

第一次启动时，选择一个 git 仓库。仓库的工作区要干净，并且检出在分支上。开发方式见 [frontend.md](notes/frontend.md) §7；改什么跑什么测试，见 [AGENTS.md](AGENTS.md)。

## 资料

设计（`notes/`）：

- [Roadmap](notes/roadmap.md)：M1 执行闭环 → M2 第二个 harness → M3 多项目 → M4 最小的全局 Angela；其余按真实使用排序
- [Architecture](notes/architecture.md)：进程、代码布局、技术选择与测试
- [Data model](notes/data-model.md)：命令、占用、turn 与消息、任务流、副作用、对话记录、额度、项目与 host
- [Harness adapter](notes/harness-adapter.md)：CLI 调用方式、执行现场、成果采集与集成（含实测）
- [Roles and tasks](notes/roles-and-tasks.md)：角色类型与具名角色、任务生命周期、Workboard
- [Manager JD](notes/manager-jd.md)：意图判断、信息过滤、任务翻译与必要时的直接对齐
- [Manager actions](notes/manager-actions.md)：向下转发、向上附件、记录、会议与对外操作
- [Frontend](notes/frontend.md)：Electron 形态、布局、前后端协议、渲染选型与现在的实现
- [Performance baseline](notes/perf-baseline.md)：GUI 性能基线的测法与参考数据
- [Ideas](notes/ideas.md)：暂不实现的小功能
- 技术路线：[#5](https://github.com/vorton-lang/Lobotomy/issues/5) 存储与权威来源的权衡，[#6](https://github.com/vorton-lang/Lobotomy/issues/6) 推荐实现（SQLite + jj + Rust 后端 + Electron 前端）

调研与背景（`notes/research/`）：

- [harness 接入与条款](notes/research/harness-interfaces.md)、[agent GUI 前端](notes/research/frontend-survey.md)；实测脚本见 `spikes/`
- [Organization Runtime](notes/research/organization-runtime.md)：早期架构笔记（角色、session、持久状态和 GUI）
- [HCI rationale](notes/research/hci-rationale.md)：人机协作研究节选
- [Market landscape](notes/research/market-landscape.md)：2026-10-01 调研草稿，产品事实与市场判断待核实

## 许可证

本仓库采用 [MIT 许可证](LICENSE)。
