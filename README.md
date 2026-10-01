# Lobotomy

> 产品探索阶段。规划与架构均为草稿，尚无可运行实现。

一个长期在线、位于现有 coding harness 之上的 agent 组织运行时：用户主要与 Manager 协作，系统承载长期角色、原生 session、可恢复状态和组织级工作界面。

**Claude Code 接入是关键要求**，不能仅用 Claude 模型 API 或提示词替代。Dots 是设计参考，用于借鉴好的设计、避免问题；不是替代或差异化目标。

## 资料

- [Manager JD](notes/manager-jd.md)：意图判断、信息过滤、任务翻译与必要时的直接对齐
- [Organization Runtime](notes/organization-runtime.md)：角色、session、持久状态和 GUI
- [HCI rationale](notes/hci-rationale.md)：人机协作研究节选
- [Market landscape](notes/market-landscape.md)：2026-10-01 调研草稿，产品事实与市场判断待核实

## 与 Ember 的关系

[Ember](https://github.com/vorton-lang/Ember) 保留模型行为、harness 与人机协作研究，以及 replay 工具和实验数据。Lobotomy 承载产品规划及后续实现。

三份原始规划笔记从 [Ember 固定版本](https://github.com/vorton-lang/Ember/tree/530debb699b90a5ae88211e5a2cea04b0c2f7f9a/notes) 原样复制，保留旧名称、草稿状态和历史假说。HCI 节选来自同版本 master-model-research.md 的 §1、§4.5、§5。原仓库文件保留，实验与回放数据未迁入。

旧文中的市场空白与产品比较不是已接受结论；未公开描述某项机制也不证明产品没有它。架构、实现栈和集成协议仍待验证。研究里的模型选型不限制产品接入 Claude Code。

迁入材料的 [MIT 版权与许可通知](THIRD_PARTY_NOTICES/Ember-MIT-LICENSE.txt) 保留；未来新增代码的许可证另定。
