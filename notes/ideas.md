# Ideas

> 暂不实现、但值得记下的小功能。每条注明来源和已知代价。

## 提交 trailer 标注来源

role 的提交自动追加 `Lobotomy-Task: <id>`、`Lobotomy-Role: <name>`，让用户在自己的 `git log` / IDE 中直接看到每个提交来自哪个任务和角色。

- 实现：只对 role 进程经 `GIT_CONFIG_*` 注入 `core.hooksPath`，由 `prepare-commit-msg` hook 读取 adapter 每轮设置的环境变量并追加 trailer。不改仓库配置，不消耗 token。
- 代价：`core.hooksPath` 会使仓库自己的 hook（husky、pre-commit 等）失效，Lobotomy 的 hook 必须负责转调它们。
- v1 不做。提交与任务的归属由成果记录直接得出，不按时间段推断（见 [roles-and-tasks.md](roles-and-tasks.md) §2.3）。
- 来源：2026-10-01 任务模型讨论。

## 中断并发送

用户发一条消息后立即中断当前 turn，运行时随即经 resume 启动新 turn，让 agent 马上看到这条消息。这相当于在 turn 进行中插话（[harness-adapter.md](harness-adapter.md) §1.1）。

- 现状：用户可以手动组合现有操作，达到同样效果：先发消息，再中断，再选择继续。运行时不阻止这种组合，也不提供专门命令。
- 加入条件：实测确认两家 CLI 在工具调用中途被中断后，会话记录完整，resume 能正常接续（harness-adapter.md §6）。
- 设想的做法：
  1. 消息进入收件箱。
  2. 运行时调用原生中断，等待 CLI 退出，turn 记为 interrupted。
  3. 运行时立即经 resume 登记新 turn。输入是一段继续说明（上一个 turn 被用户中断，请先检查 cwd），其后是排队的消息。
- 限制：只有用户可以使用，Manager 不能中断。被中断的 turn 已报告 `done` 时，任务离开执行阶段，消息按 [data-model.md](data-model.md) §4.2 排队，GUI 需要向用户说明这一点。
- 代价：中断时正在进行的那一步丢失，包括已生成的部分输出和正在运行的命令。槽位中可能留下改了一半的文件，或 `.git/index.lock` 一类的锁文件，需要 agent 在继续时自行检查。
- 来源：2026-10-03 #7 讨论。
