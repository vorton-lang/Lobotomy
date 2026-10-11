# 调研：多 agent 工具的界面与交互

> 调研快照（2026-10-11），事实可能已经变化。计划见 [ui-ux-plan.md](../ui-ux-plan.md)，本文记录依据。
> 三路网页调研：同类产品；agent 界面的指南与人机研究；桌面工作工具的视觉做法。标"文档"的来源没有发布日期，日期是读取日。只有二手来源、或原文没能读到的，标 **未核实**。性能方面的调研见 [frontend-survey.md](frontend-survey.md)。

## 1. 同类产品

与 Lobotomy 形态最接近的是"一个协调者对话 + 多个执行会话"的产品：Claude Code Projects、OpenAI dots、Devin 的 Managed Devins。

### 1.1 Claude Code（Projects、Agent View、Desktop）

- **Projects**（公开 beta，[文档](https://code.claude.com/docs/en/claude-projects)，[介绍 2026-09-17](https://claude.com/resources/articles/projects-redesigned)）：
  - 一个 coordinator 对话加若干 thread。coordinator 建的 thread 以卡片出现在用户消息下，卡片显示标题与状态，结果留在 thread 中。
  - Overview 的 thread 按固定顺序分组：Ready for review、Waiting on you（失败的 thread 也在这里）、Working、Landing、Idle、Resolved。有 thread 等用户时，Overview 按钮亮一个点。
  - 默认只在三种情况下发系统通知：coordinator 发言、thread 出错、thread 需要输入。coordinator 不转播 thread 的每一步。
  - 审批只在 thread 内进行，在协调对话里说"可以"不会传到 thread。
  - 卡片上的下一步按钮（Fix CI、Merge it 等）以用户消息的形式发进 thread。
- **Agent View**（[2026-05-11](https://claude.com/blog/agent-view-in-claude-code)）：
  - 头部计数 `Needs input: 2 | Working: 3 | Completed: 5`；分组 Pinned / Ready for review / Needs input / Working / Completed，已完成的折叠。
  - 每行：状态图标、名称、一句摘要、时长、PR 号。只有 Working 的图标有动画；Needs input 是黄色。摘要按状态写：工作中写在做什么，等待时写在问什么，完成后写结果。会话完成后时长停止计时。
  - Space 打开 Peek，不进入对话就能回答问题。
  - 曾经的 bug：权限提示期间状态仍是 Working，不计入等待，用户只能逐个点开找（#58965、#59342，已修复）。
- **Desktop**（[2026-04-14](https://claude.com/resources/articles/claude-code-desktop-redesign)）：
  - 对话有三种视图（Ctrl+O）：Normal 把工具调用折叠成摘要，Thinking 加思考过程，Verbose 显示全部。原来的 Summary 模式移除了。
  - 改动以 `+12 -1` 出现，点开是文件列表与 diff；行上可写评论，一次提交全部评论。

### 1.2 OpenAI dots 与 Codex app

- **dots**（DevDay 2026-09-29，[文档](https://learn.chatgpt.com/docs/dots)）：
  - dot 派出后台 agent、云端 thread 和 Codex 任务，用户同时可以继续与它对话。它派的任务是独立对话，用户可以打开直接指挥。
  - Activity 分 In progress / Scheduled / Completed；等决定、等登录、等批准的请求在对应任务中处理。
  - 行动规则分四档：不问就做、你说了再做、做之前先问、交给你。
  - 文档原话："A completed run doesn't by itself confirm that the requested result was achieved."
  - 用户抱怨（二手，**未核实**）：dot 反复报告进度，却不说明工作已经停了（[2026-10-01](https://www.orcarouter.ai/blog/openai-dots-first-week-reception)）；看不到也删不掉 dot 关于自己的笔记。
- **Codex app**（2026-02 发布，2026-07 并入 ChatGPT 桌面应用）：
  - 深色侧栏、浅色主区；thread 按项目分组。侧栏铃铛 Activity 列出未读、运行中、等你回复的对话。
  - 降噪：目标运行期间减少未读通知（2026-06-09）；审批请求可以先交给自动审查 agent（2026-04-23）。
  - 审查的 diff 范围可选 Unstaged / Staged / Commit / Branch / **Last turn**（[文档](https://learn.chatgpt.com/docs/code-review.md?surface=app)）。
  - 用户抱怨：审批按钮小且靠下，容易漏看；官方回复认同"approval should be an active blocker"（社区帖，**未核实**完整链接）。

### 1.3 其他

- **Cursor 3**（[2026-04-02](https://cursor.com/blog/cursor-3)）：按状态分组 Draft / Running / Needs attention / Done。Needs attention 包括被你挡住的 agent 和你还没看过的完成结果。用户抱怨：Review 面板列出整段对话的改动，看不出最近一次改了什么；列表没有颜色，只好把 emoji 写进标题；标题取自第一条 prompt，不反映当前任务。Dock 角标只计失焦期间的活动，与侧栏数字不一致。
- **GitHub Copilot / VS Code Agents**（[文档](https://code.visualstudio.com/docs/agents/run/sessions/manage-sessions)）：角标只计三类：已停止且有未读结果、等你输入、PR 的 CI 失败；仍在运行的未读会话不计入。官方建议的审查顺序是先看会话记录了解意图，再看改动，最后看检查（[2025-12-01](https://github.blog/ai-and-ml/github-copilot/how-to-orchestrate-agents-using-mission-control/)）。
- **Google Antigravity 2.0**（[2026-05-19](https://antigravity.google/blog/introducing-google-antigravity-2?hl=en)）：去掉 IDE 后遭到反对，4 天内补回醒目的"Open IDE"。计划上的评论由"Review"汇总成一条反馈，不逐条打断。完成后给 Walkthrough 摘要，附截图与录屏。一次更新移除了 Inbox，用户只能逐个页签巡查（[2026-02-27](https://discuss.ai.google.dev/t/critical-feedback-gemini-3-1-instability-agent-manager-ux-downgrade-and-vague-changelogs/127360)）。
- **Devin**（[release notes](https://docs.devin.ai/release-notes/overview)）：按状态分组 Working / Ready / Blocked / Inactive；进度压成一行当前步骤，点开看完整记录；铃铛 inbox 把"PR 等审"与"需要你输入"分开列。Devin Review 把相关改动分组、重排 hunk 顺序，问题分红黄灰三级（[2026-01-21](https://cognition.com/blog/devin-review)）。Managed Devins：子会话完成或需要输入时唤醒 coordinator（[2026-03-19](https://cognition.com/blog/devin-can-now-manage-devins)）。
- **Linear**（[Agent Interaction Guidelines](https://linear.app/developers/aig)，2025-07）：agent 始终标明自己是 agent；即时但不打扰的反馈；内部状态一眼可见、可以展开；被要求停下时立即停；agent 不承担责任。会话状态 pending / active / awaitingInput / error / complete / stale，由最后一条活动推导；10 秒无活动显示无响应，30 分钟无活动标为 stale。thought 与 action 可以是 ephemeral：下一条到达时替换，只显示"正在做什么"（[文档](https://linear.app/developers/agent-interaction)）。
- **Conductor**（[changelog](https://conductor.build/changelog)）：侧栏分 backlog / in progress / in review / done；"可合并"有明确定义（已批准、检查全绿、评论已处理、todo 完成）；专门做过一版排版调整（0.33.0）。设计者说目标是"functional, visually subtle, and chromatically warm"（[HN 2025-07-17](https://news.ycombinator.com/item?id=44594584)）。用户抱怨：隐藏中间输出后，失去"它在干活"的感觉。
- **Warp**（[文档](https://docs.warp.dev/agents/capabilities/agent-notifications/)）：通知分 Complete / Request / Error；toast 最多同时 2 个，悬停暂停计时；铃铛 mailbox 可过滤未读与错误。用户抱怨 agent 多了以后"请求 / 完成 / 阻塞"的高亮区分不够。
- **Kiro Focus Mode**（[2026-09-30](https://kiro.dev/docs/ide/experimental/focus-mode/)）：三栏 Sessions / Chat / 辅助栏；需要用户决定的事做成 Attention cards，收进标题栏铃铛，不打断阅读。
- **Zed**：选侧栏不选页签，理由是"keeping agent threads front and center"（[2026-04-22](https://zed.dev/blog/parallel-agents)）。
- 已停止或弃用：Vibe Kanban（2026-04 关停，文档仍在）、Crystal、Terragon。

### 1.4 跨产品的共同做法（3 个以上产品）

1. **四五个固定的状态词**：工作中、需要你（等回答或阻塞）、等你审、完成、失败。只有"工作中"有动画；"需要你"多用黄或橙。
2. **"等你回答"与"等你验收"分开**。Claude Projects 把失败也归进"等你"。
3. **按紧急程度排序**：需要用户的固定在最上面。
4. **通知只由三种事件触发**：需要输入、出错、你没在看时完成。
5. **中间步骤折叠但可见**：工具调用默认折叠，或只显示一行当前步骤。完全隐藏会被抱怨。
6. **每行一句摘要加时长**，完成后时长停止。
7. **评论先攒起来，一次发给 agent**。
8. **diff 可以只看上一轮**。
9. **下一步按钮代替用户发一条指令**。
10. **结果附验证证据一起交付**（截图、录屏、检查汇总）。

反复出现的抱怨：漏看审批；角标数字不准；看不出最近一轮改了什么；标题不反映当前任务；没有 inbox 只能逐个巡查；manager 报进度却不说工作已停；人同时盯住的会话约 3 个（HN 讨论、Sculptor）。

### 1.5 分歧

- **审批走哪条路**：Claude Projects 与 dots 在执行会话内处理；Devin 唤醒 coordinator 处理。Lobotomy 现在是后一种：Angela 转达，用户也可以直接打开执行者。
- **先摘要还是先 diff**：面向委派者的产品先给摘要与证据（Antigravity、Jules、Claude Projects、GitHub 的审查顺序）；面向代码作者的产品先给 diff（Codex、Conductor）。
- **列表形态**：侧栏列表是主流（Claude、Codex、Cursor、VS Code、Zed）；看板（Vibe Kanban）和网格（Cursor）是少数。

## 2. 指南与研究

### 2.1 状态与情境感知

- **按目标组织，直接给出含义。** Endsley 的 SA 导向设计：围绕目标组织信息；直接给出"这对目标意味着什么"（理解层），不让人从原始数据推；支持"接下来会怎样"（预测层）；给出所有目标的全局状态（Endsley & Jones，2012，二手核实）。
- **透明度分层。** Chen 等的 SAT 模型分三层：当前动作与计划、推理、预测。Mercado 等在多无人机实验中提高透明度后，绩效与信任校准上升，负荷与反应时没有增加（[Human Factors 2016](https://journals.sagepub.com/doi/10.1177/0018720815621206)）。
- **监督容量是瓶颈。** ParallelPilot（[2026-09-27](https://arxiv.org/abs/2609.33113)，16 人）：吞吐提高 63%，峰值多监督约一个 agent；14/16 人更喜欢它，理由是执行计划、任务依赖与介入提示更清楚。但它没有提高"能成功改变 agent 方向"的感受。作者的结论是：高层的情境感知要配上回到实现证据的低成本路径。
- **自动化多放在信息获取与分析，决策保守。** Parasuraman、Sheridan & Wickens（[2000](https://ieeexplore.ieee.org/document/844354)）：风险高、结果不确定时，决策选择的自动化程度应当低。

### 2.2 打断与通知

- **每次打断都要对应一个人的判断。** Google SRE："Every page should be actionable"；只需机械处理的不该打扰人（[SRE 书](https://sre.google/sre-book/monitoring-distributed-systems/)）。
- **假报警让人脱敏。** 临床报警 72%–99% 是假报警（二手）；Claude Code 用户批准了 93% 的权限提示（[Anthropic 2026-03-25](https://www.anthropic.com/engineering/claude-code-auto-mode)）。
- **在断点投递。** 在任务断点投递通知，挫败感更低、反应更快（Iqbal & Bailey，[CHI 2008](https://www.interruptions.net/literature/Iqbal-CHI08.pdf)）。
- **批量，但不是不通知。** 每天分三批投递时，专注与掌控感更高；完全不通知时焦虑上升（Fitz 等，[2019](https://scholars.duke.edu/publication/1402953)）。
- **三类介入。** LangChain 把人的参与分为 notify（告知）、question（要答案）、review（审一个动作），主张用专门的 inbox，不用聊天流；每项可以 accept / edit / respond（自由文本）/ ignore（[2025-01-14](https://www.langchain.com/blog/introducing-ambient-agents)）。
- **平时待在外围。** Calm Technology：只要求最少的注意力，利用外围视野（[原则](https://calmtech.institute/calm-tech-principles)）。

### 2.3 回来时的接续

- **线索不变，变化加标记。** 中断前能留线索，恢复更快；中断后线索被改动，反而妨碍恢复（Hodgetts & Jones，[2006](https://www.interruptions.net/literature/Hodgetts-JEP_LMC06.pdf)）。标出中断位置有明显帮助（Cane 等，2012）。
- **交接的结构。** Patterson 等观察了航天任务控制、核电站、铁路调度、急救调度，总结 21 种交接策略。相关的：交出方先写摘要；接收方先看历史，再看自动记录的变化；摘要包含对计划变动与应急的立场；责任转移要明确（[AHRQ](https://psnet.ahrq.gov/issue/handoff-strategies-settings-high-consequences-failure-lessons-health-care-operations)）。
- **I-PASS**：严重度、摘要、行动清单、情境与应急、接收方复述。9 个儿科住院医项目中，医疗差错下降 23%，交接时长没有显著增加（Starmer 等，NEJM 2014，二手）。
- **变化感知六要素**：谁、改了什么、在哪、何时、怎么改、为何改（Tam & Greenberg，[2006](https://cspages.ucalgary.ca/~tam/research/2006/ChangeAwareness-IJHCS06.pdf)）。
- **长等待**：完成提示写开始、结束、耗时，以及跳过和出错的步骤；登录后直接显示从上次离开处继续（[NN/g 2021-09-05](https://www.nngroup.com/articles/designing-for-waits-and-interruptions/)）。

### 2.4 审阅与验收

- **对照意图审。** Park 等（[2026-09](https://arxiv.org/abs/2609.24234)，19 名开发者加 102 个 Reddit 帖）：开发者要盯住 agent 是否自己编造或中途修改标准，以及范围蔓延；功能正确也不够。
- **批准不等于审过。** 热门仓库中 61.38% 的 AI PR 没有记录在案的 review（[EASE 2026](https://arxiv.org/abs/2605.02273)）。
- **高亮人可能改的地方。** 高亮预测会被人编辑的 token：完成更快、修改更准；高亮模型生成概率低的 token：没有益处（Vasconcelos 等，[ToCHI 2025](https://arxiv.org/abs/2302.07248)）。
- **摩擦与风险相称。** 跳过验证时给出可见信号（[Shape of AI](https://www.shapeof.ai/patterns/verification)）；发送与编辑按钮用同样的视觉权重，避免误发（[Salesforce 2024-06-26](https://www.salesforce.com/news/stories/ai-trust-patterns/)）。
- **认知强制函数有效但不受欢迎。** 先让人自己判断、再看 AI 的答案，能减少过度依赖；只加解释不能。用户最不喜欢这种做法（Buçinca 等，[CSCW 2021](https://arxiv.org/abs/2102.09692)）。

### 2.5 可争辩性与出处

- **出处紧贴论断。** 来源放在具体论断旁边，样式与正文区分；不要用分步推理充当解释（[NN/g 2025-12-12](https://www.nngroup.com/articles/explainable-ai/)）。
- **引用冻结的快照。** Linear 要求 agent 以 Agent Activity 作为用户输入的不可变快照，因为评论可以被编辑。
- **区分 AI 写的与人改过的。** IBM Carbon 的 AI label：用户改过的 AI 内容去掉 AI 样式，标签换成"revert"（[2026-09-30](https://carbondesignsystem.com/components/ai-label/usage/)）。
- **不确定性用话说。** 第一人称的"我不确定，但……"降低过度依赖并提高准确率（Kim 等，[FAccT 2024](https://arxiv.org/abs/2405.00623)）。

### 2.6 信任校准

- 自动化偏差在多任务负载下出现，新手与专家都有，训练与指令不能预防（Parasuraman & Manzey，[2010](https://journals.sagepub.com/doi/10.1177/0018720810376055)）。Lobotomy 的用户正处在"同时在做别的事"的条件下。
- 自动化越可靠，操作员越不了解关键信息，越难在需要时接管（Endsley 2017）。
- Anthropic 的数据：有经验的用户自动批准更多，中断也更多；建议给出"trustworthy visibility… along with simple intervention mechanisms"，而不是逐项批准（[2026-02-18](https://www.anthropic.com/research/measuring-agent-autonomy)）。

## 3. 视觉与实现

### 3.1 Design tokens

- **Linear 2024 重设计**（[2024-03-28](https://linear.app/now/how-we-redesigned-the-linear-ui)）：从 HSL 换到 LCH，感知均匀；主题变量从 98 个减到 3 个（base、accent、contrast）；少用品牌蓝。
- **Linear 2026 刷新**（[2026-03-12](https://linear.app/now/behind-the-latest-design-refresh)）：侧栏调暗，"Don't compete for attention you haven't earned"；图标减少、变小；边框软化，删掉多余分隔线，"Structure should be felt not seen"；灰色从冷蓝灰改为偏暖。
- **Radix Colors 的 12 级刻度**（[文档](https://www.radix-ui.com/colors/docs/palette-composition/understanding-the-scale)）：1–2 背景；3/4/5 组件常态、悬停、按下；6/7/8 边框；9/10 实色；11/12 文字。
- **Vercel Web Interface Guidelines**（[链接](https://vercel.com/design/guidelines)）：阴影至少两层；子元素圆角不大于父元素；数字列用 `tabular-nums`；尊重 `prefers-reduced-motion`；只动 `transform` 与 `opacity`；spinner 延迟 150–300ms 出现，出现后至少显示 300–500ms。

### 3.2 中文排版

- 中文 Windows 上 `system-ui` 会把拉丁字母也交给微软雅黑（[CSSWG #3658](https://lists.w3.org/Archives/Public/public-css-archive/2022Sep/0898.html)）。
- Chromium 在 Windows 上 zh-Hans 的默认 sans-serif 改为 Noto Sans SC 优先（[2024-06-14](https://chromium.googlesource.com/chromium/src/+/3ae0858daf3a68dfb7926691b8f403563b002b32%5E%21/)）。Windows 11 自 2025-03 预装 Noto CJK；微软列出的已知问题是 100% 缩放下 Chromium 渲染 CJK 发虚（[KB5053657](https://support.microsoft.com/help/5053657)）。所以字体栈要显式写，雅黑 UI 排在 Noto 前。
- `text-autospace` 在中文与西文之间自动加间隙，Chrome 140+ 支持；各家实现的初始值都是不开，要显式打开；已有空格的地方不重复加（[Chrome 2023-12-04](https://developer.chrome.com/blog/css-i18n-features)）。Electron 44 是 Chromium 152。
- 雅黑 UI 没有 600 字重：设 600 时中文落到粗体，拉丁字母是半粗，同一行粗细不一（按 CSS 字体匹配规则推断）。
- 中文不用斜体；中文段落行高 1.6–1.7；`letter-spacing` 保持 0。

### 3.3 组件、图标、动效

- **组件底座**：Base UI 1.0 于 2025-12 发布，2026-10 为 1.9；依赖少，样式用 `className` 与 `data-*` 状态，适合手写 CSS（[releases](https://base-ui.com/react/overview/releases)）。Radix 仍维护但复杂组件进展慢；React Aria 无障碍最严格但依赖面大；shadcn/ui 绑定 Tailwind。cmdk 依赖 Radix Dialog。
- **图标**：Lucide 24px 画布、2px 描边，`sideEffects: false`，具名导入每个约 1KB（约数）。整套导入（`import *`、`DynamicIcon`）会把全部图标打进包。
- **动效**：每天用几百次的交互不加动画，按键触发的动作不加动画（[Emil Kowalski](https://emilkowal.ski/ui/you-dont-need-animations)，日期**未核实**）。`@starting-style`（Chrome 117）与 `interpolate-size`（Chrome 129）可以纯 CSS 做进出场与 `height: auto` 过渡。Motion 的 `motion` 组件约 34KB。

### 3.4 Electron 桌面质感

- `titleBarStyle: 'hidden'` 加 `titleBarOverlay` 保留系统窗口按钮，Windows 与 Linux 都支持；拖拽区中的按钮收不到点击（[文档](https://www.electronjs.org/docs/latest/tutorial/custom-title-bar)）。
- `backgroundMaterial`（Mica、亚克力）只在 Windows 11 22H2+ 有效，Linux 没有对应物；社区报告过最大化与缩放问题，Electron 44 上的状态**未核实**。
- 原生手感：按钮与列表行用 `cursor: default`；`show: false` 加 `ready-to-show`，窗口背景色与页面一致，避免白闪；窗口失焦时淡化选中态（[2021](https://dev.to/vadimdemedes/making-electron-apps-feel-native-on-mac-52e8)）。

### 3.5 状态与身份

- 状态不只靠颜色：颜色、形状、文字共同表示（[Carbon](https://www.carbondesignsystem.com/patterns/status-indicator-pattern)）。
- 1 秒内不显示加载提示；单个模块用 spinner，整页 2–10 秒用骨架屏（[NN/g 2023-06-04](https://www.nngroup.com/articles/skeleton-screens/)）。Primer 的 Spinner 默认延迟 300ms。

## 4. 与 Lobotomy 立场的张力

1. **很多模式以批准 / 拒绝为中心。** 例如把"计划不经编辑就被接受"的比例当成功指标。这与"保留有价值的认知摩擦"相冲突；93% 的批准率也说明批准本身信息量很低。支持 Lobotomy 立场的有：LangChain 的 respond（自由文本）、Linear 的 elicitation、Anthropic"不逐项批准"的建议。
2. **把逐步放权当增长目标**（Smashing 的 Autonomy Dial），与"只在有证据时才自动化"相冲突。Endsley 与 Onnasch 都警告：自主度越高，失效时情境感知损失越大。
3. **数字置信度。** Microsoft 与 Smashing 建议显示确定程度；PAIR、Kim 等、Vasconcelos 等的结果都指向自然语言表述或"人会改哪里"更有效。后者与 Lobotomy"区分事实、解读与假设"一致。
4. **展示推理过程。** Linear 主张推理可以展开；NN/g 建议不要用分步推理当解释。
5. **主动开口会增加打断**（Microsoft 的"nudging more than notifying"、ParallelPilot 的黄色闪烁），与"低带宽情境感知"相冲突。ParallelPilot 是短时实验室任务，不能直接套用到"经常离开"的场景。
6. **抽象层让人远离证据。** 所有事情经过 Angela 时，必须保留到会话、diff 与检查输出的低成本路径（roadmap.md"接触成果是一等入口"）。
7. **"接受"不等于"监督过"。** 只记"已验收"不能说明人看过证据。是否记录验收时看过的证据，是待决问题，不在本次计划中。

## 5. 未核实与空白

- dots 与 Codex 的部分用户抱怨只有二手来源。
- Codex cloud 任务列表的状态文案、Antigravity 1.x Inbox 的样子、Factory 的排序规则。
- Emil Kowalski 文章的日期与具体时长取值。
- Mica 相关缺陷在 Electron 44 上的状态；HarmonyOS Sans、MiSans 的许可。
- 各家的字体与配色规范，Linear 以外没有一手材料。
