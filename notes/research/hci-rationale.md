# HCI rationale: Ember research excerpts

> Status: research working hypotheses, extracted 2026-10-01. 原文保留其研究状态；不是已验证的产品原则或验收标准。
>
> Source: [master-model-research.md](https://github.com/vorton-lang/Ember/blob/530debb699b90a5ae88211e5a2cea04b0c2f7f9a/notes/master-model-research.md), commit `530debb699b90a5ae88211e5a2cea04b0c2f7f9a`, blob `8d93bac86563590cb9cb62e9b8dbe109f7e494f9`.
>
> Selection: §1 当前研究模型、§4.5 Product harness / UX、§5 Interaction / HCI。以下内容按原顺序逐段保留；编号与分隔线沿用原文。实验结果、历史会话细节、模型选型和 replay 改造计划未迁入。

## 1. 当前研究模型

### 1.1 Master 不是单模型 trait，而是 human–model coupled regime

此前把 master 主要写成模型的 **long-horizon collaborative meta-control**。这仍描述了模型侧能力，但现在看不完整。

更准确的研究对象是：

> **在一个真实但欠定义的问题上，人持续提供机器无法机械恢复的现实、价值、因果线索与方向梯度；模型持续吸收这些输入，重建问题表示，并把它们向工程后果传播。双方共同维护并提高一个 shared world model。**

因此 master trajectory 不是“模型连续很多轮都很聪明”，而是 **human–model state transition** 的序列。

一个典型循环是：

```text
欠定义的真实问题
→ 模型形成 provisional world model
→ 人注入 latent knowledge / value gradient / correction
→ 模型 belief revision / abstraction lift
→ 工程后果展开
→ 新现实或新价值判断再次进入
→ shared world model 上升
```

后期很多真正重要的 insight 已经无法干净归因给“人”或“模型”任一方。一个人类输入可能只有一个词，但如果它让整个因果模型重建，信息增益可以极高。

因此后续研究不再只问：

> “master model 会做什么？”

而要问：

> **什么条件使 human–model system 进入、维持或掉出 master regime？模型侧哪些 policy 能扩大这个 regime 的出现概率？**

### 1.2 人类侧：不是 prompt quality，而是不可替代的信息源

高价值的人类输入不是“写得更长”或“更像 specification”。

最关键的是四类模型无法仅从 repository / tools / tests 中恢复的东西：

- **Latent knowledge**：历史原因、真实使用习惯、组织背景、未记录的事故；
- **Value gradient**：什么算变好、什么复杂度值得付、哪些东西“正确但讨厌”；
- **Lived constraints**：长期工程经验、实际工作流、规模感、性能与维护直觉；
- **Structured ambiguity**：已经发现方向和矛盾，但故意不把推理闭合，把真正未知的缺口留给联合系统。

典型高杠杆输入不是完整答案，而更像：

> “你这个局部我认，但沿它推下去好像会撞到另一个更大的东西；我有一个还没想清楚的方向……”

这处在两个坏极端之间：

- 输入太少：模型 autopilot；
- specification 太完整：模型退化为 executor。

当前候选概念：

> **高熵输入应按 world-model update 的幅度理解，不按 token 数理解。**

### 1.3 模型侧：能做的不是“教育用户”，而是维护高杠杆交互界面

用户本身是否愿意思考、是否拥有相关经验，是外生变量。既存在高主观能动性的用户，也存在只希望批准 / 获取结果的用户；没有必要把“把后一类用户训练成前一类”当研究目标。

模型不能要求用户学会“如何成为好用户”。它能优化的是自己的 interaction policy，使已有的人类 latent knowledge 更容易、以更低成本进入 shared state。

当前拆成六个候选能力：

| 能力 | 问题 |
|---|---|
| **Detect** | 能否意识到当前结论依赖一个可能存在于用户侧的关键隐变量，而不是直接闭合 |
| **Value** | 能否判断这个未知值不值得打扰人，而不是所有 ambiguity 都追问 |
| **Elicit** | 能否以低认知成本暴露关键假设 / 提出高区分度问题 |
| **Uptake** | 用户给出新信息后，是否真的修改 world model，而不是只说“理解了” |
| **Propagate** | world-model update 是否传导到后续工程结论、计划和实现 |
| **Pace** | 能否在执行、继续探索、停下来让人介入之间正确分配 initiative |

关键不是“问更多问题”，而是：

> **把当前最关键、最可证伪的假设露在表面，让用户知道自己有什么值得说。**

这是一种 **agency-preserving elicitation**，不是 clarification checklist。

### 1.4 Capability 与 usability 重新定义

**Master capability** 主要看模型本身能不能：

- framing repair；
- ontology formation / refactoring；
- latent invariant discovery；
- second-order reasoning；
- principle induction；
- belief revision；
- downstream propagation。

**Master usability** 主要看真实互动中能不能稳定地：

- 在正确时机调用这些能力；
- 暴露而不是掩埋关键假设；
- 管理 judgment bandwidth；
- 保留用户 agency；
- 吸收 sparse / high-leverage feedback；
- 避免 premature closure；
- 维持 situation awareness；
- 不把用户压成 approval endpoint。

当前工作性判断：

> 现代模型的 peak master cognition 可能比历史体感暗示的更普遍；真正拉开长期体验的，很可能是 activation、uptake 与 orchestration。

### 1.5 四个旧 latent capabilities 仍成立，但 collaborative control 需要展开

| Capability | 核心问题 | 典型表面行为 |
|---|---|---|
| **Representation mobility** | 能否离开当前 framing、重选表示空间 | framing repair、ontology refactoring、teleological reframing、second-order consequence |
| **Hierarchical abstraction** | 能否区分 goal / principle / constraint / mechanism / implementation | principle induction、invariant / mechanism 分离 |
| **Sparse-feedback amplification** | 能否把短 control signal 放大成高层重构 | correction leverage、短纠偏后重建问题空间 |
| **Collaborative control** | 知道什么时候做多少、哪些判断留给人 | Detect / Value / Elicit / Pace、judgment bandwidth、agency preservation |

其中 Uptake / Propagate 横跨 representation mobility 与 sparse-feedback amplification，可能比之前想的更接近基础 reasoning / state-update capability。

### 1.6 乘法假说更新

旧式：

`master usefulness ≈ representation mobility × abstraction quality × feedback amplification × collaborative control`

仍然有用，但现在应明确 human side：

```text
realized master trajectory
≈ human latent contribution
× model uptake / propagation
× interaction orchestration
× time
```

这不是评分公式，只表达：任一因子接近零，联合系统都会塌。

这也解释了两个对称失败：

- 模型很强但 interaction policy 把用户压成审批器 → 人类输入熵持续下降；
- 用户有很强的洞见但模型只局部 patch / 复述 → 人的认知投入没有杠杆，最终也会停止投入。

### 1.7 当前因果模型

```text
model capacity / pretraining
    ↓
latent cognition
    ↓
post-training / default policy
    ↓
system prompt / inference scaffold / harness
    ↓
interaction policy
    ↕
human latent state / value gradient
    ↓
shared world-model transitions
    ↓
realized master trajectory
```

因此接下来必须区分：

- **基础能力缺失**：明确给出新前提后，模型仍不能重建 frame / 推导后果；
- **policy gating**：模型会，但默认不 Detect / Elicit / Pace；
- **prompt-level elicitation**：少量原则或 few-shot 就能稳定恢复；
- **inference scaffold**：需要显式 deliberation / controller 才稳定；
- **post-training trait**：prompt 能短暂改变，但跨 domain / 长上下文 / 多轮后不稳定。

---

### 4.5 Product harness / UX

公开 system prompt 单独移植不足以复现 worker 化，因此真实 product effect 可能来自组合：

`system prompt + tool contract + context construction + reminders + planner/worker roles + continuation policy + UX + matching post-training`

特别值得关注：

- bounded-execution framing；
- tool loop 是否奖励局部闭合；
- context pruning 是否只保留 local task state；
- 是否频繁把用户角色压成 approve / reject；
- async / timeout 是否切断高熵反馈窗口。

---

## 5. Interaction / HCI：从“frictionless assistance”转向 agency-preserving amplification

这里的目标不是提高 engagement，也不是强迫用户投入更多。

关键区分是：

> **minimize wasted user effort, maximize consequential human judgment**

也可以写成：

> **消掉操作摩擦，保留有价值的认知摩擦。**

### 5.1 Disruptive interaction

大部分现代 assistant / agent 的默认优化目标是：只要能继续，就继续；能自动完成就减少用户介入。

在开放工程问题里，这可能形成负反馈：

```text
模型把用户当审批器
→ 用户越来越只审批
→ 模型获得的 latent state 越来越少
→ 模型更依赖自己的 framing
→ 用户越来越看不懂
→ 更只能审批
```

反方向：

```text
模型暴露当前关键假设
→ 用户发现值得纠正的地方
→ 注入 latent knowledge / value gradient
→ 模型真的 uptake 并改变后续路线
→ 用户发现自己的判断有杠杆
→ 更愿意继续思考和干预
```

因此真正要优化的是：

> **模型能否维护一个让人的判断值得投入的界面。**

### 5.2 不是 explainability，而是 contestability

长、完整、组织漂亮的 explanation 不一定保护 agency，甚至可能让反驳成本更高。

更重要的是把“接缝”露出来：

- 哪些是事实；
- 哪些是模型解释；
- 哪个关键假设一旦改变会推翻哪些结论。

好的 master reply 不一定更短，但应该让用户很容易知道：

> **我有什么值得说，以及这句话会改变什么。**

### 5.3 Situation awareness

用户连续几轮只说 `ok` / `继续` 并不自动代表失败。

真正危险的是用户已经：

- 不知道模型下一步为什么做；
- 不知道当前哪些假设仍未决；
- 不知道什么时候应该阻止；
- 无法用低带宽描述项目当前 shape。

因此 master interaction 要维持的是低带宽但正确的 shared project model，而不是让人追踪全部实现细节。

### 5.4 用户特质是条件，不是训练目标

存在不同用户：

- 有强 latent knowledge、愿意参与 framing；
- 只希望获取结果；
- 两者随任务切换。

当前研究不打算证明“模型能把第二类用户变成第一类”。

评估时应固定 human latent state / response packet，研究：

> **给定同一个可利用的人类判断，模型是否能发现、调用、吸收并传播它。**

---

完整研究与实验依据继续在 [Ember 原文](https://github.com/vorton-lang/Ember/blob/530debb699b90a5ae88211e5a2cea04b0c2f7f9a/notes/master-model-research.md) 维护。
