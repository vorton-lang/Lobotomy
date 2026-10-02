# Lobotomy

> 产品探索阶段。规划与架构均为草稿，尚无可运行实现。

一个长期在线、位于现有 coding harness 之上的 agent 组织运行时：用户主要与 Manager 协作，系统承载长期角色、原生 session、可恢复状态和组织级工作界面。

**Claude Code 接入是关键要求**，不能仅用 Claude 模型 API 或提示词替代。Dots 是设计参考，用于借鉴好的设计、避免问题；不是替代或差异化目标。

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
