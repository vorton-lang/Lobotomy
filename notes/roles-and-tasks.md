# 角色与任务

> Status: 设计决定 v0.3（2026-10-03）
> 前置：[harness-adapter.md](harness-adapter.md)、[manager-actions.md](manager-actions.md)
> v0.2 按 [#5](https://github.com/vorton-lang/Lobotomy/issues/5) 已确认的"持久工单 + 受控状态机"方向修订：执行轮（attempt）、完成条件版本化、证据绑定具体成果。
> v0.3 按 [#7 的讨论补充](https://github.com/vorton-lang/Lobotomy/issues/7#issuecomment-5953401457)修订：attempt 与 turn 的关系、`done` 与采集的顺序、验收的原子边界。三项均已确认，见 [data-model.md](data-model.md) §4、§5。

## 1. 角色

### 1.1 角色类型与具名角色

- **角色类型**是模板，规定职责、指令、工具权限与槽位策略：`manager`、`tech_lead`、`worker`、`reviewer`。
- **具名角色**是实例，绑定一个类型、一个 harness 和一个模型。名字沿用 Lobotomy Corporation 的设定，最多 11 个：Angela，以及 10 位 Sephirah（Malkuth、Yesod、Hod、Netzach、Tiphereth A、Tiphereth B、Gebura、Chesed、Binah、Hokma）。
- **名字只是标识**，给用户一点 role play 的感觉，不影响工作。名字出现在 GUI、消息标题（`【Binah】`）、会议中的 @、以及"等 Binah"这类状态里。角色指令中只告知 role 自己的名字，不写入游戏角色的性格。

### 1.2 v1 人员

| 名字 | 类型 | harness | 工作目录 |
|---|---|---|---|
| Angela | manager | Claude | 在 host，全局一个；不占槽位（data-model.md §10.6，M4） |
| Binah | tech_lead | Claude | 槽位 `tl` |
| Malkuth | worker | Codex | 槽位 `worker` |
| Yesod | reviewer | Codex | 在固定的候选成果上单独物化的审查现场 |

- 表中的 harness 是默认值。每个 role 都能换 harness，Angela 也一样（harness-adapter.md §0，用户确认，2026-10-09）。
- Angela 原来是每个项目一个，工作目录是用户的主仓库（只读）。现在全局只有一个，属于用户（**语义更新**，data-model.md §10.6）。

### 1.3 类型职责

- **manager**：用户的全局协作者，也是默认入口。用户经它管理各个项目，也可以直接打开其他 role 的窗口。直接通路是常设的二等入口，不是临时方案；用户自然选择，不强制（**语义更新**，[#23](https://github.com/vorton-lang/Lobotomy/issues/23)，用户确认，2026-10-09）。原来写的是"用户与组织之间的控制面"。职责见 [manager-jd.md](manager-jd.md)，动作见 [manager-actions.md](manager-actions.md)。
- **tech_lead**：找问题、提质疑、做架构层面的调研。由 Manager 派任务，也可被 Manager 临时咨询。代码审查不是它的职责。
- **worker**：执行任务。`done` 只提交候选成果，不代表任务完成。
- **reviewer**：对照完成条件和代码质量审查一份固定的候选成果，可以跑测试；只给出结论（通过 / 要求修改）和具体意见，不改代码。结论绑定所审查的成果与完成条件版本。
  - 审查对象是已 pin 的不可变成果，在其上单独物化审查现场；Reviewer 在现场中的任何改动都不会进入成果（harness-adapter.md §3）。
  - 每个任务新开一个会话，同一任务的多轮审查共用：前者避免受过去审查的先入之见影响，后者让它能核对自己上一轮的意见是否已处理。由运行时机械轮换。

### 1.4 没有 Angela 也能运转

Lobotomy 离开 Angela 应当仍然可用：会非常难用，但控制流能完整跑通。

- 所有控制流命令（建任务、验收、退回、放弃、答复问题、确认对外操作、发起会议、修改共识）用户都能在 GUI 中直接执行，与 Angela 走同一命令入口（#6 §1）。
- Angela 缺席时，原本进入她收件箱的内容（下层汇报、审查结论、阻塞问题）直接呈现给用户，不经过滤。
- 这也覆盖了 Angela 所在额度耗尽的降级场景（[#4](https://github.com/vorton-lang/Lobotomy/issues/4)），并让控制流可以在没有 LLM Manager 的情况下做确定性的端到端测试。
- Angela 与其他 role 的位置和功能不同：直接面对用户、过滤信息、持有 ledger、不占槽位。能否整合进统一的 role 模型，到实现 Angela 时再判断。

## 2. 任务

### 2.1 定义

- 任务是 Manager 派给某个 role 的一项工作，带可验证的完成条件，由 Workboard 跟踪。
- 任务属于 Lobotomy 内部，不等于 GitHub issue。"做了 70%"这类状态在 GitHub 上表达不了，很多任务也小到不值得开 issue。任务可以关联 issue 或 PR。
- 层级只有两层：目标（ledger 中的 `goal`）→ 任务。拆分粒度由 Manager 判断，标准是完成条件可验证、工作量以小时计而非以天计。
- 每个 role 同一时间最多一个进行中的任务，其余排队。
- **任务 ID 稳定**，跨 turn、会话更换和下线保持不变。
- **执行轮（attempt）**：每次进入执行就开一个 attempt，包括任务开始、验证失败、退回、重开。一个 attempt 可以包含多个 turn；同一 attempt 内接续时，运行时只新建 turn，不新建 attempt。重开任务建立新 attempt，并保留已关闭的历史。旧 attempt 迟到的 `done` 不能完成新 attempt。见 [data-model.md](data-model.md) §4.1。
- **完成条件版本化**：条件被修改后，旧证据只对旧版本有效。代码任务与研究任务采用各自相应的证据；没有代码不等于自动免审。修改的处理与证据的作废条件见 data-model.md §4.4、§4.5。
- **离开执行阶段后，发给执行者的消息排队**，任务回到执行时再投递。任务关闭时不再投递；还有未投递的用户消息时，验收要用户明确放弃它们（data-model.md §4.2，**语义更新**，#14）。

### 2.2 生命周期

v1 优先稳定而不是吞吐：任务真正完成前，所有参与者都不转去做别的。

```text
排队 → 执行 → 验证 → 审查 → 验收 → 完成
        ↑      │失败  │要求修改 │退回
        └──────┴──────┴────────┘
  执行阶段可进出"阻塞"；任何阶段都可"放弃"
```

- **阶段、用户暂停、阻塞原因三者分开记录。** 额度恢复不解除用户暂停；暂停期间的产出保留，但不自动推进。
- **所有状态变化都经运行时命令完成**（GUI 与 agent 同一入口），由命令检查权限、当前执行轮与前置条件，并连同依据一并持久保存。

| 阶段 / 变化 | 推动方 |
|---|---|
| 新建 | Manager `assign`，或用户在 GUI 中直接建（任务说明只有用户原话，没有 Manager 解读） |
| 排队 → 执行 | 运行时：执行者空闲时，把队首任务的说明发给它 |
| 执行 ⇄ 阻塞 | 执行者 `org_report(status: blocked, blocked_on)`；卡在等用户时自动挂到"等你决定"。用户的回复绑定到执行者的下一个 turn 时解除（data-model.md §9.2） |
| 执行 → 验证 | 执行者 `org_report(status: done)`。`done` 可能早于 CLI 退出：运行时先保存采集意图并保持占用，CLI 退出后再采集并固定成果，作为候选成果（harness-adapter.md §4.1） |
| 验证 | 运行时：采集范围完整、集成无未处理冲突、检查命令通过。检查在固定的验证现场运行（data-model.md §5）。未通过则把原因直接发回执行者，不经 Manager |
| 验证 → 审查 | 运行时：把候选成果交给 Reviewer |
| 审查 → 验收 / 退回 | Reviewer 给出结论。通过则进入验收；要求修改时由 Manager 判断哪些现在改、哪些暂缓（可转为 issue），退回时按 ID 引用 Reviewer 意见原文。暂缓意见、风险豁免与最终验收分别留痕，"暂缓了全部意见"不能记作"审查通过" |
| 验收 → 完成 / 退回 | Manager 判断是否满足完成条件；例外验收如实留痕。验收即发布：运行时以 CAS 把成果发布进集成版本，集成版本已前进时先重新验证。验收决定、CAS 与任务关闭在同一个 SQLite 事务中提交；CAS 失败时任务不关闭（harness-adapter.md §4.2） |
| 放弃 | Manager 或用户。关闭本轮及后续调度，保留成果与证据。未验收的成果从不进入集成版本，放弃不涉及撤销代码 |

- **参与者 = 执行者 + Reviewer。** 从开始到完成或放弃，运行时不给参与者派别的任务。Manager 临时咨询 TL 不会使 TL 成为参与者。
- **审查轮数上限默认 3 轮，由运行时执行。** 超过后不再自动退回，要求 Manager 在提议开会、验收、放弃三者中选一个。重开任务不会静默重置审查预算，也不继承旧结论。
- **任务范围的约束自动失效。** ledger 中 `scope: task:<id>` 的 `constraint` 在任务关闭时失效。

### 2.3 任务与成果

每份成果在采集时就记录来源：task、attempt、基线与采集范围（见 harness-adapter.md §4.1）。"这个任务改了什么"以及审查范围由成果记录直接得出，不靠时间段推断。发布到集成版本的提交用 trailer 标注任务与执行者（harness-adapter.md §4.3）。

## 3. Workboard

以下全部由任务记录、运行时状态与 ledger 机械投影：

```text
等你决定
  · Malkuth 的检查命令用什么？                         ▸ 来源

Binah    调研：并发语义的几种方案                进行中 · 正在跑 turn 2 分钟
Malkuth  实现 Codex adapter                     审查中（执行轮 1 · 审查第 1 轮）
         候选成果 14:20 采集 · 验证通过
Yesod    审查：实现 Codex adapter               进行中
排队     Malkuth：补上 contract test

暂缓 4 条 · 最近完成 3 个 · CI ✓
```

任务状态（语义）与 role 当前活动（运行时事实：正在跑 turn / 等待消息 / 空闲）分开显示。

## 4. 工具

| 使用方 | 工具 |
|---|---|
| Manager | `assign(role, title, done_when, quote, context?, ledger?, interpretation, links?)`；`send_to_role(..., task_id?)` 追加说明；`accept(task_id)`；`send_back(task_id, items: [id], reason)`；`abandon(task_id, reason)` |
| 执行者 | `org_report(title, body, status: progress \| blocked \| done, blocked_on?, trial?)`；`trial` 是 done 时可选的试用建议，只保存，不执行（frontend.md §8） |
| Reviewer | `org_report(..., verdict: approve \| changes_requested, items)` |
| 用户（GUI） | Manager 的全部控制流命令（建任务、验收、退回、放弃），以及调整队列、重开、暂停某个 role；立即生效，Manager 只收到通知 |

## 5. 未决

- 通知策略见 [manager-actions.md](manager-actions.md) §6。
- #5 中列出的每类对象的完整契约（研究任务的交付证据、完成条件修改后在途工作的处理、多个旧基线成果的整合责任等）尚未逐项回答。
- 业务对象、命令与唯一约束见 [data-model.md](data-model.md)。
