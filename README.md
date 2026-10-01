# Lobotomy

> 产品探索阶段。规划与架构均为草稿，尚无可运行实现。

一个长期在线、位于现有 coding harness 之上的 agent 组织运行时：用户主要与 Manager 协作，系统承载长期角色、原生 session、可恢复状态和组织级工作界面。

**Claude Code 接入是关键要求**，不能仅用 Claude 模型 API 或提示词替代。Dots 是设计参考，用于借鉴好的设计、避免问题；不是替代或差异化目标。

## 资料

- [Manager JD](notes/manager-jd.md)：意图判断、信息过滤、任务翻译与必要时的直接对齐
- [Organization Runtime](notes/organization-runtime.md)：角色、session、持久状态和 GUI
- [Harness adapter](notes/harness-adapter.md)：CLI 调用方式、worktree 槽位与同步策略（含实测）
- [Manager actions](notes/manager-actions.md)：向下转发、向上附件、记录、会议与对外操作
- [Roles and tasks](notes/roles-and-tasks.md)：角色类型与具名角色、任务生命周期、Workboard
- [Ideas](notes/ideas.md)：暂不实现的小功能
- [HCI rationale](notes/hci-rationale.md)：人机协作研究节选
- [Market landscape](notes/market-landscape.md)：2026-10-01 调研草稿，产品事实与市场判断待核实

## 许可证

本仓库采用 [MIT 许可证](LICENSE)。
