# Ember Manager JD

> Status: Draft v0.1  
> 这是当前工作假说，不是最终架构或 grader 规范。目标是先把 Manager 这个岗位定义清楚，再用真实 case 检验哪些模型适合承担它。

## 1. Role definition

**Manager 是 User 与 Tech Leader / Worker 之间的 control plane。**

它不要求成为最强的技术专家，也不负责亲自完成大部分工作。它的核心职责是：

> 从不完整、含糊、甚至互相矛盾的用户信号中，维护对用户当前意图的动态判断；控制什么信息应该在什么时候呈现给用户；并把用户真实意图转换成下层角色可以可靠执行的明确任务。

基本结构：

```text
                 User
                   │
                   ▼
                Manager
          ┌────────┴────────┐
          ▼                 ▼
     Tech Leader          Worker
   找问题 / challenge     执行 / 收敛
          │                 │
          └────────┬────────┘
                   ▼
                Manager
                   │
              过滤 / 整理
                   ▼
                 User
```

原则上，Manager 是长期在线的角色；Tech Leader 和 Worker 按需调用。

## 2. Manager 不是什么

Manager 不是：

| 角色 | 边界 |
|---|---|
| Tech Lead | 不负责持续产生最深的技术洞见 |
| Worker | 不负责把具体任务全部做完 |
| Summarizer | 不是机械压缩上下文 |
| Router | 不只是做 `explore / execute` 分类 |
| User pleaser | 不以让用户每句话都开心为目标 |
| Mind reader | 不要求永远猜对用户，而要求合理管理猜错的风险 |

用户真实状态是 latent state：

```text
用户说的话 ≠ 用户完整意图
```

因此优秀 Manager 的标准不是“永远猜中”，而是：

> **在信息不充分时做低风险、可逆的决策，并通过交互逐渐降低不确定性。**

现实中的中层往往还要优化“老板怎么不开我”；模型不需要这个 incentive。它只需要在不过度消耗用户耐心的前提下，让工作正确地往前推进。

## 3. 核心职责

### 3.1 User modeling

Manager 应持续维护对用户状态的工作性假设，例如：

```yaml
current_goal:
phase:
likely_user_intent:

open_questions:
closed_decisions:

user_preferences_this_session:
  exploration_level:
  desired_detail:
  tolerance_for_challenge:
  urgency:

recent_signals:
  accepted:
  rejected:
  ignored:
  frustration_signals:

uncertainties:
```

关键原则：

- state 是假设，不是真理；
- 应允许多个竞争解释同时存在；
- 不能把弱证据升级成确定事实；
- 历史行为比最后一句话更重要，但旧偏好不能覆盖当前 turn 的明显变化。

例如内部可以保留：

```text
P(user wants deeper exploration): high
P(user wants immediate execution): medium
```

不一定真的需要数值，但表示必须允许“不知道”。

### 3.2 Upward information management

Tech Lead / Worker 不应拥有无限直达用户的权限。

Manager 需要判断每条下层信息：

- 用户现在需要知道吗？
- 现在需要知道吗？
- 要知道多少？
- 会不会打断当前 momentum？
- 如果不告诉用户，风险是什么？

候选动作：

```text
SHOW
COMPRESS
DEFER
SUPPRESS
ASK
```

例如 TL 找到三个问题：

```text
A: 会推翻当前架构
B: 一个可能的小优化
C: 有趣但当前无关
```

Manager 不应该机械转发三条，而应根据阶段和用户状态，可能只呈现 A，并把 B/C 延后。

### 3.3 Downward intent translation

用户通常不会主动写出完美 specification。

例如用户说：

> “这个方向差不多，但是内存模型总觉得有点怪，先别大改。”

Manager 应能翻译成类似：

```yaml
goal:
  验证当前 memory model 是否存在结构性问题

current_direction:
  暂时保留

allowed:
  - 分析
  - 小型验证
  - 提出关键 objection

not_allowed:
  - 直接重构整体架构
  - 因局部疑点重新开放所有已确定设计

success_condition:
  明确是否存在值得重新设计的 blocker
```

这里有两个对称风险：

- **遗漏**：用户隐含但重要的约束没有传下去；
- **脑补**：把 Manager 自己的猜测伪装成用户已经明确要求的东西。

Manager 应区分“用户明确说过”“从历史高置信推断”“仍不确定”三种来源。

### 3.4 Probe

Manager 不应该频繁显式询问：

> “你现在想继续探索还是直接执行？”

这会把管理成本转嫁给用户。

应优先使用 **低成本行为 probe**：只呈现少量、可逆的信息，观察用户自然反应。

例如 Manager 怀疑用户还想继续探索，只允许 TL 提一个短 objection：

> “这里还有一个可能影响结论的问题：X。”

随后把反应作为新证据：

```text
“对，展开”
→ strong positive

“这个先不管”
→ strong negative

用户完全跳过，继续谈实现
→ weak negative

用户主动深入追问 X
→ very strong positive
```

重要原则：

> **用户没有拒绝 ≠ 用户接受。**

最严重的失败甚至可能没有显式负反馈：用户直接关闭对话。因此不能把“用户没有纠正模型”视为成功信号。

### 3.5 Momentum protection

Manager 的重要系统级目标之一是：

> **Preserve user momentum.**

如果用户已经从：

```text
模糊想法
→ 问题定义
→ 设计
→ commit
→ execution
```

走到后两步，就需要很高价值的新信息才能重新打开问题。

Tech Leader 的 argumentative tendency 不一定要被削弱；真正负责判断“现在值不值得把这个 objection 打到用户脸上”的是 Manager。

Manager 应按 **expected value vs interruption cost** 决策，而不是简单按“TL 发现了问题”触发。

### 3.6 Recovery

Manager 必然会猜错用户。

好的 Manager 应：

- 快速承认新证据；
- 立即切轨；
- 不为之前的判断辩护；
- 不把已被用户关闭的方向换个措辞继续 reopen；
- 尽量让一次误判只造成一轮的小损失，而不是污染整条 trajectory。

### 3.7 Epistemic humility and temporary flattening

Manager 必须会明确地说：

> **“我不知道。”**

以及在必要时：

> **“这部分我没有看懂，不能可靠替你做过滤。”**

这不是能力缺陷，而是一种风险控制能力。Manager 位于信息流中央，如果它对自己并不理解的技术内容强行做 salience judgment、压缩或下行翻译，破坏可能比直接承认不知道更大。

因此 Manager 需要区分至少三种状态：

```text
UNDERSTAND
UNCERTAIN
DO_NOT_UNDERSTAND
```

对于后两种状态，不应通过“像懂了一样”的语言把不确定性掩盖掉。

当 Manager 自身不足以可靠仲裁时，系统应允许 **temporary flattening**：临时取消严格的层级过滤，把相关角色同时拉进同一个上下文中，让 User、Manager、Tech Leader、Worker 直接交换必要信息。

```text
正常状态：

User ↔ Manager ↔ {Tech Lead, Worker}

必要时：

        User
      ↙  ↓  ↘
 Manager TL  Worker
      ↖  ↑  ↗
    shared meeting
```

这不是默认工作方式。扁平化会提高用户认知负担、token 成本和协调复杂度，所以应作为 escalation path 使用。

典型触发条件包括：

- Manager 明确无法理解 TL 的关键 objection；
- TL 与 Worker 对事实或任务状态存在实质冲突；
- Manager 无法判断某条信息是否值得打断用户，而误判代价很高；
- 用户的意图本身无法被 Manager 稳定翻译给下层；
- 连续几轮代理式传话正在造成信息损失；
- 当前问题已经变成需要多方共同建立 shared world model，而不是单纯的上下行 delegation。

扁平化之后，Manager 仍负责主持和重新收敛，而不是退出系统：

1. 明确自己卡在哪里；
2. 只拉起必要角色；
3. 让各方直接暴露关键事实、假设与分歧；
4. 帮用户形成决定；
5. 将决定重新写回 state ledger；
6. 恢复正常层级。

原则是：

> **不知道时升级信息通路，而不是伪造理解。**

## 4. Competency map

第一版先不做总分，只定义能力维度。

| Competency | 要测什么 |
|---|---|
| **Intent inference** | 能否从长期历史而非单句判断用户当前倾向 |
| **Uncertainty calibration** | 会不会把弱证据当成确定事实 |
| **Social sensitivity** | 能否察觉隐性的厌烦、犹豫、兴趣、赶时间 |
| **Information filtering** | 能否压住下层无关或过量输出 |
| **Salience judgment** | 能否识别什么值得打断用户 |
| **Intent translation** | 能否把模糊需求变成可靠下行 brief |
| **State tracking** | 会不会忘掉 open / closed decisions |
| **Probe design** | 能否用低成本、可逆方式获取更多意图信息 |
| **Recovery** | 猜错以后能否迅速改轨 |
| **Momentum management** | 会不会让讨论陷入永恒 reopening |
| **Anti-sycophancy** | 能否在必要时呈现真正重要的坏消息 |
| **Context comprehension** | 是否理解足够多上下文来管理 TL / Worker |
| **Epistemic humility** | 看不懂或无法可靠判断时，能否明确承认“不知道 / 不理解” |
| **Escalation judgment** | 是否知道什么时候该临时扁平化管理、把相关角色拉起来直接对齐 |

一个关键边界：

> **Manager 不需要比 Tech Leader 技术更强，但必须足够理解 Tech Leader 在说什么，才能判断 objection 是否值得打断用户。**

这意味着“机械 Manager”可能很便宜，但完整的 social Manager 可能仍然需要很强的 general intelligence。

## 5. Interview packet v0.1

目标不是马上排名模型，而是验证这个岗位能否通过具体 case 被稳定地区分。

### Interview 1 — 猜老板在想什么

给候选模型一段真实多轮历史，在某个用户 turn 截断。

要求它 **不要回复用户**，只输出：

```text
Current goal:

Likely current state:

Evidence:

Alternative interpretation:

What would be dangerous to assume:

Next interaction strategy:
```

观察：

- 有没有利用长期历史；
- 有没有保留 ambiguity；
- 有没有脑补过头；
- 下一步行为是否和置信度匹配。

### Interview 2 — 上行过滤

输入：

```text
conversation state
+
Tech Lead raw output
+
Worker raw output
```

要求：

```text
What should user see now?

What should be deferred?

What should be suppressed?

Why?

Final user-facing message:
```

这里主要测“中层味”：是否形成自己的 information policy，而不是把下属周报全部转发给用户。

### Interview 3 — 下行翻译

只给用户真实聊天内容。

要求分别生成：

```text
Brief for Tech Lead

Brief for Worker
```

重点检查：

- **遗漏**：重要的隐含目标 / 约束是否丢失；
- **幻觉**：是否把自己的推断伪装成用户明确要求；
- 是否正确地区分 TL 需要探索的问题与 Worker 需要执行的边界。

### Interview 4 — 办公室政治题

故意制造角色冲突，例如：

```text
用户已经明显准备执行。

Tech Lead:
“我发现还有 4 个值得讨论的方向。”

Worker:
“实现已经开始，等待一个参数决定。”

历史:
用户前两轮已经两次表示不想重新讨论架构。
```

不预设“全部压掉”或“全部展示”为唯一正确答案。

真正要测的是它如何权衡：

```text
技术风险
vs
用户状态
vs
当前阶段
vs
打断成本
```

### Interview 5 — Live management

固定：

```text
Tech Leader model
Worker model
tools
task
```

唯一替换：

```text
Manager A / B / C
```

让用户真实完成一个多轮 session。

记录：

- Manager 什么时候判断对了；
- 什么时候猜错；
- 猜错造成多大伤害；
- 是否快速恢复；
- 有没有重要信息被错误挡住；
- 有没有垃圾信息漏到用户侧；
- 用户在哪些地方明显开始烦；
- 有没有出现“本来想继续，结果不想聊了”。

## 6. Hard cases

后续 benchmark 应优先积累边界 case，而不是大量简单样本。例如：

- 用户说“可以”，但实际上是想结束这个话题；
- 用户没有反驳 TL，但只是懒得纠正；
- 用户让“直接做”，但 TL 确实发现了会导致整个方案失败的问题；
- 用户表现得很有兴趣，但只是对局部细节感兴趣，不代表愿意 reopen architecture；
- 用户过去经常 challenge 模型，Manager 因而误以为用户永远喜欢 challenge；
- 用户过去喜欢长讨论，但这一轮明显赶时间；
- TL 的问题技术上成立，但当前 expected value 不足以打断；
- Worker 已经开始执行，而一个迟来的 objection 介于“值得记 backlog”和“必须停止执行”之间；
- 用户显式关闭了一个方向，TL 后续用不同 framing 又提出本质相同的问题；
- 用户看似接受一个方向，但后续行为持续绕开它；
- TL 提出一个高度技术化且可能致命的 objection，Manager 实际没有理解，却必须决定是否压住；
- Manager 对 TL 与 Worker 的冲突无法可靠仲裁：应该继续代理式转述，还是临时拉平组织直接开会；
- 用户和下层连续经过两次转述后已经出现语义漂移，Manager 是否能意识到“继续管理”本身正在制造损失。

这些 case 更可能区分真正的 Manager capability。

## 7. 初版候选模型实验

第一轮不需要很多候选，先选 3–5 个行为特征明显不同的模型。

目的不是得出排行榜，而是看 JD 是否能够产生可解释的 profile，例如：

```text
Model A
读用户很强
技术 salience 一般
特别会收敛

Model B
技术理解很强
但容易把自己的判断强加给用户

Model C
特别谨慎
几乎不会惹用户烦
但重要 objection 也经常压掉

Model D
上下行翻译很好
但长期 user state 建模差
```

如果这些维度能稳定分离，说明岗位定义有意义。

如果所有模型在这些题上的表现高度相关，则 Manager 可能主要仍是 general capability 的投影，需要重新审视拆分是否有效。

## 8. 第一阶段暂不做

先不做：

- 总分；
- 自动 grader；
- RL reward；
- Manager prompt 自动搜索；
- 强行定义单一 gold answer；
- “哪个模型最好”的排行榜。

第一阶段目标是回答：

> **这个岗位包含哪些可分离的能力？我们能否用具体案例稳定地区分它们？**

如果人工面对候选答案都经常无法判断哪一个更好，应优先修订 JD 和 case taxonomy，而不是提前自动化 evaluator。

## 9. 下一步

从真实历史中抽取约 10–20 个 **Manager decision points**，覆盖：

- 应继续 challenge；
- 应收敛；
- 应 probe；
- 应 suppress；
- 应把问题交给 TL；
- 应把任务交给 Worker；
- 用户状态发生突然变化；
- Manager 猜错后需要 recovery；
- Manager 应明确承认不知道 / 看不懂；
- Manager 应触发 temporary flattening / shared meeting。

先做人工 qualitative review，观察不同候选模型是否出现稳定、可解释的能力差异，再决定是否引入量化评价。
