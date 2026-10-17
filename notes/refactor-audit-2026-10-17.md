# 第 1 步重构审查（2026-10-17）

## 范围与依据

审查固定在 `eec33c47eec1dcc3eb6e664c8b092a8a06831a67`，重构前基线为 `273b373809ab45c943900d1f2be51bb80e5795d4`。两者之间有 176 个变更文件。开始时没有开放 PR；该 main 提交的 CI（39026936711）已经通过。这次是独立审查，不接手每小时 CI 修复。

依据是当前 `AGENTS.md`、`notes/architecture.md`、`tech-plan.md`、`worker-interface.md`、相关数据与界面设计，以及 #86 正文和全部 14 条评论。#86 的最后一条实现评论早于实际 HEAD：`0b724725` 已删除本地执行者路径，并把 runner 回归迁到 worker 宿主；`eec33c47` 又清理了界面的本地 role 假设。

本轮按个人 agent Angela、共用 `lobotomy-agent`、外部 worker 宿主及环境审查。第 1 步保留项目库；全局库、图与新事件在第 2 步。远端 WebSocket、`put_blob`、steer、扩展键、独立一致性测试包及额度完整拆分明确推后。不会用已退出的 jj、采集、预览架构要求当前代码。

四路并行审查分别负责测试留存，架构与状态/权限/生命周期，技术债与重复实现，以及注释。另有独立审查者复查本次高风险差异。盘点覆盖全部 238 个受版本控制文件：232 个 UTF-8 文本、6 个图标二进制。静态扫描覆盖全仓文本；源码按入口、状态迁移和副作用路径审读，测试结合历史修复核查。注释扫描包含源码、测试、历史 SQL、spikes、前端工具及 CI；不是把正常设计文档机械删除，也不声称每个历史文件都已逐行证明正确。

## 测试留存

确切政策来自 `AGENTS.md`“测试的目的”、`tech-plan.md` §8 和 `971878f` 的提交说明。现有旧测试保留两类：修 bug 的提交新增或实质修改的测试；在 CI 失败过，随后由同时修改生产代码的提交修好的测试。机械改名或签名调整不算。`971878f` 的 188 项退役、84 项留存名单曾由用户确认。

新代码的长期测试政策仍待定；§7 同时明确要求 worker 内部契约测试。因此没有把“新的测试还没有失败过”当作删除理由。未删除现存测试或有效断言，未放宽测试，未修改受版本控制的测试运行器。

本轮恢复两条被整段删除连带带走的回归：

- `9f543fb` 修复了 turn 的 MCP token 被序列化给 GUI，新增的断言后来由 `63e92c8` 搬进搜索测试，最后随该测试在 `971878f` 中删除。恢复为 `lobotomyd/tests/gui.rs` 的独立 Angela 用例，验证真实非空 token、目标 turn 存在，以及直接序列化、thread、snapshot 和 `continue` 返回值均不泄露。后者也覆盖 `55ef58c` 的实际修复。
- `9f543fb` 同时修复双击创建任务会重复提交。原断言随 `m1.spec.ts` 整条旧流水线退役，但 `NewTask/useCommand` 仍在。现在用模拟 WebSocket 延迟回执，验证同一表单双击只发一次命令，不恢复已退出的本地执行者流程。

`0b724725` 迁到 worker 的六项 runner 行为仍有有效检查；`eec33c47` 删除的交付说明与项目输入框断言对应已经删除的 UI。

## 已处理的发现

下列行号指审查基线，便于回查原缺陷；修复与回归保存在本 PR。

| 编号/级别 | 基线位置 | 证据与影响 | 处理及验证行为 |
|---|---|---|---|
| A1 / P1 | `worker/src/environment.rs:165`；`lobotomyd/src/workers.rs:134` | `git diff --name-only` 把中文及控制字符转义成带引号的文本；字面目录前缀匹配失败。rename 检测还只列目的路径，移出受保护目录时漏掉源路径。临时真实 Git 仓库可复现两种绕过。 | 用 `--no-renames --name-only -z` 读原始路径，不 trim；真实环境回归覆盖中文、空格、Unix 的换行/制表符/引号/尾空格及移出受保护目录。非 UTF-8 路径报错，不降级为不完整授权。 |
| A2 / P1 | `lobotomyd/src/gui/command.rs:168–196` | GUI 的 `SendMessage`/`SendBack` 重试被换成新 request ID；还丢弃引用并覆盖 role。 | 保留 ID 与引用，校验 role，已确认投递的消息不重发。真实 WebSocket 回归检查两端各两条消息、三个 turn、重复退回、错误 role 和无效引用拒绝。引用留存不等于已经跨进程传入上下文，见 D5。 |
| A3 / P1 | `lobotomyd/src/workers.rs:292–307,656–670` | create/revise 的事件可先于 RPC 回包。创建前的事件被丢弃；新 attempt 的事件也可能被当作旧 attempt，仅推进 seq，登记后停在 submitted。 | 登记后按持久化 seq 重新 subscribe，对账快照并补事件。回归强制在回包前应用末态，检查 create 与第二版需求的 revise 最终均 delivered，项目均 accepting。 |
| A4 / P1 | `agent/src/runner.rs:347–360` | 先等 stdout EOF 才等 CLI 退出。主 CLI 退出、后代继承 stdout 或 stderr 时，turn 可永远 working。新回归在旧实现先失败。 | 同时监控 CLI 退出；确认主进程退出后结束所属进程树，再读完缓冲输出。`finish` 仍在 EndTurn 前确认整树退出。回归保留最终回复、completed 结果并检查后代结束；覆盖两种管道继承。 |
| A5 / P2 | `lobotomyd/src/workers.rs:183–197,236` | RPC 等响应期间持有 stdin 锁；shutdown 在 deadline 之外等同一把锁。取消、写失败或超时还会遗留 pending 条目。 | 写完即释放锁；RAII 清理 pending；关闭按同一 deadline 取锁。回归检查已发请求仍可取锁、取消与 broken pipe 清理，以及持锁时关闭期限。未修改 RPC 超时常量。 |
| T1 / P2 | `971878f` 删除的 GUI 搜索测试 | 实际 token 泄露修复的断言被连带删除。 | 恢复独立 GUI/序列化安全回归，历史依据见上节。 |
| T2 / P2 | `971878f` 删除的任务创建 e2e | 双击重复创建的实际修复仍适用，但断言被连带删除。 | 恢复小范围模拟 WebSocket 回归，历史依据见上节。 |
| C1 / P3 | `frontend/package.json:31`；`frontend/src/format.ts` | `@pierre/diffs` 随 DiffView 删除后已无调用；`jsonNumber` 全仓只剩定义。 | 删除依赖及其独占锁记录、未用函数；前端类型、单测与构建验证。 |

## 注释与当前文档

删除或准确改写的代表项：

- `agent/src/{instance,lib}.rs`、`lobotomyd/src/instance.rs` 不再声称后端运行项目执行者。共享实例结构和锁的寿命约束仍保留。
- `agent/src/launch.rs` 去掉“Codex 先读完输入才写输出”的不可靠保证；保留 stderr 防阻塞原因。
- `core/src/item.rs:59` 删除不存在的 startup sweep 保证，保留先写文件再入库避免记录缺文件的崩溃顺序。
- 公共 `harness/src/process.rs` 以调用者/拥有进程表述主体；保留 Windows Job、进程组、父线程死亡信号及句柄继承约束。
- 删除失效的 `screenshots.spec.ts` 引用、过时阶段标签和复述函数名的说明；纠正 probe 模式数及假 CLI 大 stderr 注释的实际时序。
- README 与 `tech-plan.md` 原过渡段明确区分已经接入 worker 的实现和仍待实际验收；没有把旧过渡状态写成当前能力。

保留了设计原因、不变量、并发和安全边界、许可证、生成文件说明及工具指令。旧 SQL 描述当时 schema 的注释作为历史保留。worker 协议要求 close 清理工作区，不能因实现尚欠缺就删掉规范。

## 未解决的架构与契约问题

这些问题未以临时业务规则绕过，也没有新开公开 issue。位置仍指审查基线。

| 编号/级别 | 文件/行与证据 | 影响、建议及本轮结果 |
|---|---|---|
| D1 / P1 | `worker/src/task.rs:326–349`；`lobotomyd/src/workers.rs:576–595` | forge 合并、实例 Accept、worker.db 已采用记录跨三处提交。合并成功但响应或本地记账丢失时没有完整对账；Accept 后、set_adopted 前崩溃还可恢复成 adopted，但重试又因不是 delivered 被拒。需持久化采用意图，按 PR/head 的权威合并事实恢复，再幂等完成本地关闭；要测试两个提交窗口。未做跨库状态机重写。 |
| D2 / P1 | `lobotomyd/src/gui/command.rs:182–186`、`workers.rs:568–611`；对照 `core/src/task.rs:989–1007` | worker Accept 丢掉 `dropping`，合并前不检查未投递用户消息，最后关闭会取消它们。转发失败后验收可能让用户的话未经确认消失。需复用精确 dropping/调用者校验，并明确采用在途时新消息的处理，避免只补一次前置检查仍留副作用竞态。未自创采用锁与消息语义。 |
| D3 / P2 | `lobotomyd/src/workers.rs:539–553`；`worker/src/task.rs:362–367` | worker 已接受消息、后端尚未记录转发时结果不确定。重试从当前 question 重新生成 question_id，可能与首次参数不同，遭严格幂等校验拒绝。应持久化首次转发参数或按 worker 快照消息回执对账。A2 只修正常成功后的重试，不声称覆盖此窗口。 |
| D4 / 待决 | `frontend/src/components/TaskPanel.tsx:72`；`core/src/task.rs:931–944`；`worker/src/task.rs:211–224` | 已关闭 worker 任务仍显示重开，但旧 Reopen 只改项目 phase；同 ID create 返回旧 closed attempt，无法重新执行。需决定重开产生新 worker task 身份，还是协议允许明确重开；未擅自扩展 close 后的协议语义。 |
| D5 / P2 | `lobotomyd/src/workers.rs:546–552`；`worker/src/task.rs:622` | 原消息引用可在项目保存，却未传给执行者：forward 只发 body，refs 为空，worker 重建消息时 reply_to=None。需定义引用跨实例的格式、取原文及版本边界。 |
| D6 / P2 | `core/src/worker.rs:262–275,300–350`；`core/tests/worker_tasks.rs:91–103` | authoritative snapshot 不推进事件水位，旧事件重放仍可改变当前状态/phase并唤醒 Angela；测试甚至允许相同 seq 的 working 覆盖 delivered 快照。需区分当前投影水位与历史落库水位，或将补发在对账事务中处理；不能只跳 seq 而丢历史。未改既有恢复语义。 |
| D7 / 待决 | `notes/roles-and-tasks.md:55`；`lobotomyd/src/workers.rs:270–310` | 旧 role 文档要求同一 role 串行；当前 dispatch 每轮可再派一项，同一 host 按 task 并发。#86 的并发上限仍待登录刷新实测。需确认 worker 的并发配置、调度与界面表示，不据旧架构强加串行。 |
| D8 / 待验证 | `agent/src/launch.rs:64` | 先写完整 stdin 才进入 drive。CLI 退出、后代继承 stdin 且不读取时，大输入仍可能在输出/退出监控前阻塞。A4 未覆盖此窗口；需故障回归后再确定输入投递与 InputDelivered 的并发实现。 |

第 1 步已经承诺、实现评论也已明示尚未完成的还有 L1 内存上限、close 后工作区清理、progress/read_log。实际验收仍是让 Angela 派 2–3 个真实任务，经历推送、PR、CI 内循环、按版本采用、重启及停止；本次假 CLI/forge 回归不代替该验收。

## 验证与限制

本地在指定保存的云端环境执行。运行相关新回归、workspace 全量测试、Rust 格式与两种 feature 的全 targets Clippy、GUI 类型漂移检查、前端格式/lint/类型/单测/构建，以及全量适用 e2e。最终命令结果与准确 PR head 的 Windows/Ubuntu CI 链接记录在 PR 描述中。

独立复查覆盖路径授权、消息幂等、注册前事件、CLI 退出排管道及 RPC 锁/取消。新回归检查实际行为，没有用删除断言、忽略失败或改运行器制造通过。非代码交付、真实 forge 写入验收及 Windows 本地验证不在这次云端假件测试结果中冒称已完成。
