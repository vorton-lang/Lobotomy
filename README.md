# Lobotomy

> 状态：全局的 Manager Angela 和 worker 层可用。Angela 派出的任务由 worker 在自己的 clone 里完成，开 PR、跑 CI，再按版本合并。方向见[技术方案](notes/tech-plan.md)（[#86](https://github.com/vorton-lang/Lobotomy/issues/86)），已知问题和下一步见其中 §7。

一个长期在线、位于现有 coding harness 之上的 agent 组织运行时：用户主要与 Manager 协作，系统承载长期角色、原生 session、可恢复状态和组织级工作界面。

**Claude Code 接入是关键要求**，不能仅用 Claude 模型 API 或提示词替代。Dots 是设计参考，用于借鉴好的设计、避免问题；不是替代或差异化目标。

## 现在能做什么

执行者由 host 目录中 `workers.toml` 配置的 worker 提供，每个条目指定项目名和 harness，可选模型。worker 在宿主准备的工作区中使用官方 Codex CLI 或 Claude Code。在 GUI 中，你可以：

- 给项目配置的 worker 建任务，查看任务面板中的交付说明、PR 与检查；
- 在「等你决定」的卡片中回答 worker 提出的问题；
- 退回交付物让 worker 返工；代码交付物按指定版本合并，报告类交付物验收后关闭任务；
- 同时管理多个项目：在左侧的项目列表中新建、切换、归档。
- 在「全部」中与 Angela 对话。她能读所有项目的现状，替你派活、验收、退回、放弃和暂停任务；你暂停的任务只有你能恢复。她回复中引用的原话和成果由运行时显示，点开成果是任务面板。
- 项目停下来等你、或者活干完了时，Angela 会自己醒来，写一条合并的消息。她要你决定的问题列在「等你决定」中。
- 你可以"回复"她的某一条消息，指定你在回什么；她转达你的话时会带上它，你的消息下写明转给了谁。
- 右侧栏的"共识"列出她记下的目标、决定和约束，各附你的原话。

独立审查角色 Yesod 还没有实现。worker 的执行过程不在界面显示。

## 从源码运行

需要 Rust、Node.js、git，以及已经登录的 Codex CLI 或 Claude Code。

1. `cd frontend && npm install`（第一次，或依赖变了之后）
2. `npm start`：编译后端、构建前端，然后打开应用

应用发现后端已经在运行时，会连上它，不会启动新编译的那个。更新代码后，先在托盘中退出 Lobotomy，再运行 `npm start`。

第一次启动时，选择一个 git 仓库。后端只记下它的路径，不检查、不复制，也不写入这个仓库。开发方式见 [frontend.md](notes/frontend.md) §7；改什么跑什么测试，见 [AGENTS.md](AGENTS.md)。

## 资料

设计（`notes/`）：

- [Tech plan](notes/tech-plan.md)：#86 的新方向：分层、worker、forge 与治理、编排层（图、事件、Angela）、界面、现有代码的去留与顺序
- [Worker interface](notes/worker-interface.md)：编排层与 worker 之间 task 级的接口：操作、状态、事件、交付物与证据、凭据
- [Roadmap](notes/roadmap.md)：M1 执行闭环 → M2 第二个 harness → M3 多项目 → M4 最小的全局 Angela；其余按真实使用排序
- [Architecture](notes/architecture.md)：进程、代码布局、技术选择与测试
- [Data model](notes/data-model.md)：命令、占用、turn 与消息、任务流、副作用、对话记录、额度、项目与 host
- [Harness adapter](notes/harness-adapter.md)：CLI 调用方式、执行现场、成果与验收（含实测）
- [Roles and tasks](notes/roles-and-tasks.md)：角色类型与具名角色、任务生命周期、Workboard
- [Manager actions](notes/manager-actions.md)：向下转发、向上附件、记录、会议与对外操作
- Manager 的现行指令：[prompts/manager.md](backend/crates/lobotomyd/src/prompts/manager.md)（意图判断、信息过滤、任务翻译）
- [Frontend](notes/frontend.md)：Electron 形态、布局、前后端协议、渲染选型与现在的实现
- [UI/UX plan](notes/ui-ux-plan.md)：界面改进专项的原则、视觉方向与分阶段计划
- [Performance baseline](notes/perf-baseline.md)：GUI 性能基线的测法与参考数据
- [Ideas](notes/ideas.md)：暂不实现的小功能
- [Maintaining](notes/maintaining.md)：维护小组的共识：碰到待验收的改动、CI 失败、罕见故障这类情况时的做法与注意点
- 技术路线：[#5](https://github.com/vorton-lang/Lobotomy/issues/5) 存储与权威来源的权衡，[#6](https://github.com/vorton-lang/Lobotomy/issues/6) 推荐实现（SQLite + jj + Rust 后端 + Electron 前端；jj 私有存储已随 #86 删除）

调研与背景（`notes/research/`）：

- [harness 接入与条款](notes/research/harness-interfaces.md)、[agent GUI 前端](notes/research/frontend-survey.md)；实测脚本见 `spikes/`
- [UI/UX survey](notes/research/ui-ux-survey.md)：多 agent 工具的界面与交互、同类产品用户的痛点（2026-10-11/12 快照）
- [Organization Runtime](notes/research/organization-runtime.md)：早期架构笔记（角色、session、持久状态和 GUI）
- [HCI rationale](notes/research/hci-rationale.md)：人机协作研究节选
- [Market landscape](notes/research/market-landscape.md)：2026-10-01 调研草稿，产品事实与市场判断待核实

## 许可证

本仓库采用 [MIT 许可证](LICENSE)。
