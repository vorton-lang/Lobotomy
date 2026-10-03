# Roadmap

> Status: 规划 v0.1（2026-10-02）
> 依据：[#6](https://github.com/vorton-lang/Lobotomy/issues/6) "先交付一条可验证闭环"；[roles-and-tasks.md](roles-and-tasks.md) §1.4 "没有 Angela 也能运转"。

## 原则

- **每个里程碑都端到端可用。** 先打通一条完整链路，再增加对象种类和调度能力。
- **Angela 放在最后。** 前三个里程碑由用户直接充当 Manager，控制流不依赖 LLM Manager，"Angela 可以缺席"在一开始就得到验证。Angela 与其他 role 位置不同，能否整合进统一 role 模型，到 M4 再判断。
- **性能基线从 M1 开始**（[frontend.md](frontend.md) §5）。
- **用 Lobotomy 开发它自己**，在主要流程实现之后再考虑。
- 在 Windows 上开发，不依赖平台特有行为；正式发版前集中修 Linux 问题。

## 里程碑

| 里程碑 | 内容 |
|---|---|
| **M1 执行闭环** | 用户在 GUI 中给 Malkuth 建任务 → 执行一轮 → 采集并固定成果 → 验证 → 用户验收 → 发布 → 物化预览 → GUI 显示。包括 Rust 后端骨架、SQLite 命令入口、Codex adapter、jj 采集与发布、最简 Electron 界面、性能基线框架 |
| **M2 审查** | Yesod 加入：独立审查现场、审查结论、最多 3 轮退回，由用户验收 |
| **M3 Binah 与会议** | Claude adapter；研究类任务按非代码证据验收；会议（无 Angela 时由用户主持） |
| **M4 Angela** | 对话、向下转发原话、向上附件与暂缓、共识面板、`ask_user`、由状态触发的通知；评估统一 role 模型 |

M1 开工前，先在 [#7](https://github.com/vorton-lang/Lobotomy/issues/7) 定下数据地基：业务对象、命令入口与对话记录模型。

M4 之后考虑：

- 用 Lobotomy 开发它自己；
- 在官方应用中接管与交还 role 的会话（[#1](https://github.com/vorton-lang/Lobotomy/issues/1)）；
- 多项目管理：项目内的管理方式不变，外面加一层多项目隔离（[data-model.md](data-model.md) §10）。
