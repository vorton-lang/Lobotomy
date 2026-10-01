# Ember Organization Runtime & UX Notes

> Status: early architecture notes  
> 目标不是先做一个“多 agent 聊天工具”，而是定义一个长期在线的组织运行时：它位于各类模型专用 harness 之上，承载角色、session、状态、事件、项目世界与用户控制面。

## 1. Why a higher-level runtime is needed

当前 Worker / Tech Leader 往往被锁在模型特定的专用 harness 中，例如 Claude Code、Codex、API harness 或其他 CLI / UI。

因此需要一个更高层的系统来统一承载：

- 用户与 Manager 的主交互；
- Manager / Tech Leader / Worker 等角色；
- 每个角色正在使用的 harness 与 model；
- 各角色的 active session；
- Manager 私有的 user model / ledger；
- 项目级共享状态；
- GitHub、文件、issue、task、artifact 等外部世界；
- 全局事件与任务状态；
- 长期在线的后台运行能力。

形态上可能看起来像 IM，但 IM 只是表层。真正的 source of truth 应该是一个 **persistent organization runtime**。

候选分层：

```text
Model
  ↓
Harness
  ↓
Harness Adapter
  ↓
Persistent Agent Session
  ↓
Organization Runtime
  ↓
Manager / coordination policy
  ↓
GUI
```

## 2. Role, Session, Durable State must be separated

“保存 role context”这个说法太粗。

一个角色至少应拆成：

```text
Role
├── Durable State
├── Active Session
└── Session History / Provenance
```

### Role

角色的长期职责与 identity，例如：

- Manager；
- Tech Leader；
- Worker。

### Active Session

当前 harness 原生维持的工作上下文。

优先继续使用原 session，因为其中可能包含：

- harness 原生连续性；
- 模型已经形成的局部工作状态；
- provider prompt cache / cached input 的价格优势；
- 暂时仍有价值的细碎上下文；
- harness 自己维护的 tool / environment 状态。

### Durable State

由 Ember 自己长期保存，用于：

- session 崩溃恢复；
- 换模型；
- 换 harness；
- 主动新开 session；
- 长期组织记忆。

它不应等价于完整聊天历史，而更像 **semantic checkpoint**。

例如 Tech Leader：

```yaml
role: tech_leader

mission:
current_problem:
working_model:

accepted_facts:
decisions:
open_questions:
active_hypotheses:
important_artifacts:
do_not_reopen:
handoff_notes:

session_provenance:
  previous_session:
  checkpoint_at:
```

Worker 可以维护：

```yaml
task:
implementation_status:
blockers:
dirty_files:
tests:
next_step:
```

Manager 则有独立的私有 user model / state ledger。

## 3. Event log is not the same thing as role state

完整历史仍应长期保存，但用途不同。

建议区分：

```text
Event Log
= 完整可追溯历史，用于 replay / debugging / research / audit

Durable Role State
= 可恢复的语义状态

Active Session
= 当前真正工作的上下文
```

Event log 可记录：

```text
USER_MESSAGE
AGENT_MESSAGE
TASK_ASSIGNED
TASK_COMPLETED
DECISION_MADE
DECISION_REOPENED
STATE_UPDATED
FILE_CHANGED
COMMIT_CREATED
AGENT_SLEEP
AGENT_WAKE
ESCALATION_STARTED
MEETING_STARTED
MEETING_ENDED
SESSION_CHECKPOINTED
SESSION_ROTATED
MODEL_SWITCHED
```

GUI 中的 IM 视图只是 event stream 的一种 projection，不应成为底层数据模型。

## 4. Session continuity should be the default

最好的 context policy 不是“全部保存”或“每次 selective rebuild”，而是：

> **能安全续用旧 session 时优先续用；只有换 session 时才依赖 Ember 保存的 durable state 与 selective context materialization。**

这样可以同时保留：

- 原 session 的 continuity；
- provider cache 带来的 input 降价；
- harness 内部局部状态；
- 已经形成的工作 attractor。

也避免过早要求系统解决“完美 selective context”这个困难问题。

## 5. Session rotation is a separate control problem

是否继续老 session，应该综合考虑。

### 继续老 session 的收益

```text
native continuity
+ cache discount
+ local working memory
+ environment continuity
```

### 继续老 session 的成本

```text
stale context
+ wrong attractor / persona drift
+ 已关闭问题残留
+ unrelated history
+ context window pressure
```

另外还有明确的 rotation 原因：

- 换模型；
- 换 harness；
- session 崩溃；
- 当前工作阶段发生显著变化；
- 当前轨迹已经被错误假设或噪声污染；
- 需要干净地重新建立 persona / role state。

可以把它理解成 **session lease**：

> 默认续租；必要时 rotate。

第一版无需自动决定。可以由用户 / Manager 手动：

```text
Continue Session
Checkpoint & New Session
Switch Model
```

每一次人工选择都可以留下数据，未来再研究自动 session rotation policy。

## 6. Context Materializer

只有在新 session 启动时，Context Materializer 才成为关键路径。

候选输入：

```text
Durable role state
+ selected recent events
+ relevant decisions
+ relevant artifacts / repo state
+ task-specific context
        ↓
   New Session
```

原则：

- 不默认把完整 event log 灌进新 session；
- selective context 应服务于当前 role / task；
- context selection 同时考虑质量、长度与 provider cache 经济性；
- 应保留 provenance，知道一条信息是从哪里带入新 session 的。

第一版 context selection 可以手动或半手动，不要求立即自动化。

## 7. Harness adapters and capability differences

不同 harness 的能力不一致，adapter 应显式声明 capability，例如：

```text
supports_resume
supports_non_user_messages
supports_system_update
supports_tool_injection
supports_background_run
supports_streaming
supports_cancel
supports_checkpoint
```

三类典型情况：

### Native resumable session

最好处理。保存 session id / working directory / environment 后继续 resume。

### Stateless API

Session 逻辑归 Ember 所有。需要时由 Ember materialize context。

### Interactive-only harness

最困难。

如果 harness 只支持 user turn，Manager / peer agent 发来的内部消息最终可能只能作为 synthetic user input 注入。

上层仍应保留真实来源：

```text
source: manager
target: tech_leader
type: internal_instruction
```

adapter 再负责降级为 harness 能接受的形式。

这是 harness 的硬限制，不能假装不存在。

## 8. Persistent service, not a command-line experiment

长期目标不是：

> 启动 harness → 做一轮 → 退出。

而是：

> **runtime 永远在线，用户只是偶尔接入这个已经存在的组织。**

候选进程结构：

```text
emberd
├── agent supervisor
├── session manager
├── context materializer
├── scheduler
├── event store
├── state store
├── harness processes
└── web server
```

用户下线后：

- Worker 可以继续执行；
- Tech Leader 可以等待 Worker；
- Manager 可以收结果；
- 是否主动打扰用户由 notification policy 决定。

第二天重新打开 GUI 时，看到的是昨天仍然存在的“办公室”，而不是一套重新初始化的 agents。

## 9. GitHub is part of the shared world, not the whole world

GitHub 很适合承载 durable project state：

- source；
- design docs；
- issues；
- commits；
- experiment artifacts。

但不适合承载全部 live organization state，例如：

- Manager 对用户当前意图的临时判断；
- TL 尚未确认的 objection；
- Worker 当前 70% 完成的任务；
- agent session handle；
- “用户刚才可能开始不耐烦”这类弱信号。

因此需要至少两个世界：

```text
Durable project world
GitHub / files / artifacts

Live organizational world
DB / event log / agent sessions / manager ledger
```

两者互相引用，但不强行统一成一个存储层。

## 10. GUI is a core research/product surface

GUI 不是后期给 CLI 套的壳。

如果内部抽象先围绕“一堆聊天窗口”长出来，最终很容易退化成 Discord for bots。

正常情况下，用户不应该亲自管理 agent 通信。

核心原则：

> **用户主要看到组织状态，而不是 agent 通信。**

### Main conversation

默认只和 Manager 交互。

### Workboard

显示：

- 谁在做什么；
- 哪些任务 blocked；
- 哪些 decision 等用户；
- 哪些后台任务正在运行。

例如：

```text
Decision needed
  Memory model: A vs B

Blocked
  Worker waiting for allocator semantics

Tech Lead
  Found one architecture-level objection

Background
  2 tasks running
```

### Agent Inspector

需要时才展开某个角色的内部状态：

- 当前 model；
- harness；
- active session；
- session age；
- context usage；
- cache 状态；
- current task；
- durable state；
- recent internal events。

同时提供：

```text
Continue Session
Checkpoint & New Session
Switch Model
```

### Meeting / Temporary Flattening

作为一级 UX 操作支持：

```text
Bring Everyone In
Start Meeting
```

临时把 User / Manager / Tech Leader / Worker 拉到 shared context 直接对齐。

结束后：

- Manager 总结；
- 更新 state ledger；
- 各角色分别恢复自己的 session；
- 系统重新回到正常层级。

## 11. Session rotation UX

第一版不需要自动判断切 session 时机。

Agent Inspector 可以直接显示：

```text
Tech Leader
Model: ...
Session age: ...
Context: ...
Cached prefix: ...

Current phase:
...

[Continue Session]
[Checkpoint & New Session]
[Switch Model]
```

新开 session 时，可以允许用户 / Manager 选择 carry-over：

```text
Carry into next session:

✓ current goal
✓ decisions
✓ open objections
✓ relevant repo state
✓ recent turns
□ full conversation
```

未来 Manager 可以建议：

> “这个 session 已经带有大量上一阶段内容，我建议 checkpoint 后重开。”

但第一版仍保留人工决定。

## 12. UX principles

当前至少有以下原则：

1. **Manager 是默认入口，不让用户直接承担组织协调。**
2. **下层 activity 默认是低噪声 ambient information。**
3. **组织状态优先于 agent chat。**
4. **内部消息和完整 trace 默认属于 debug / inspector view。**
5. **高风险结构变化必须可见且可控，例如 session rotation / model switch。**
6. **扁平化是显式升级路径，而不是默认多人群聊。**
7. **尽量保持用户 momentum，不要求用户频繁做管理决策。**
8. **系统应允许深入 inspection，但不强迫用户理解底层 orchestration。**

最终目标不是“让用户管理一群 agents”，而是：

> **让用户感觉自己在和一个运转良好的组织合作。**

## 13. Open questions

目前最重要的 open questions：

### Persistent agent session 的最小公分母是什么？

不同 harness 差异很大，需要定义哪些能力是 runtime 必须抽象出来的，哪些只能 capability-dependent。

### Synthetic user turn 会如何污染 role state？

当 harness 不支持 internal / peer messages 时，如何最小化把 Manager 指令伪装成 user input 带来的 persona 与 alignment 偏移？

### Session rotation policy 应如何定义？

需要综合：

- context quality；
- stale history；
- persona attractor；
- cache price；
- native session continuity；
- context window；
- task phase。

第一版可手动，后续再从真实选择中学习。

### Selective context 应怎么 materialize？

不仅是“什么相关”，还要考虑：

- 哪些信息适合进入 role 的长期 state；
- 哪些只应作为当前 task context；
- 哪些应该保留为可查但不主动注入；
- 哪些信息具有高污染风险。

### GUI 应暴露多少内部组织状态？

太少会失去控制感与可调试性；太多又会把 Manager 的工作重新甩给用户。

### Background operation 与 notification policy

系统不下线以后，需要决定：

- 哪些任务可自行继续；
- 什么情况必须停下来等用户；
- 什么结果值得主动通知；
- 如何避免后台 agents 自发扩展 scope。

## 14. Current working architecture

```text
                    GUI
                     │
                     ▼
             Organization Runtime
        ┌────────────┼─────────────┐
        │            │             │
     Manager     Event/State     Session Manager
        │          Stores             │
        │                              ▼
        │                      Context Materializer
        │                              │
   ┌────┴───────────────┐       Harness Adapters
   ▼                    ▼              │
Tech Leader           Worker        Models / CLIs
```

核心研究对象逐渐分成两条：

> **Manager = organization control policy**

> **GUI = user-facing control surface**

二者都属于 UX 的核心。底层模型 / harness 再强，如果最终要求用户自己协调 agents、过滤下层通信、判断什么时候重开 session，那么系统仍然失败。
