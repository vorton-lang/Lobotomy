# Ideas

> 暂不实现、但值得记下的小功能。每条注明来源和已知代价。

## 提交 trailer 标注来源

role 的提交自动追加 `Lobotomy-Task: <id>`、`Lobotomy-Role: <name>`，让用户在自己的 `git log` / IDE 中直接看到每个提交来自哪个任务和角色。

- 实现：只对 role 进程经 `GIT_CONFIG_*` 注入 `core.hooksPath`，由 `prepare-commit-msg` hook 读取 adapter 每轮设置的环境变量并追加 trailer。不改仓库配置，不消耗 token。
- 代价：`core.hooksPath` 会使仓库自己的 hook（husky、pre-commit 等）失效，Lobotomy 的 hook 必须负责转调它们。
- v1 不做。提交与任务的归属由成果记录直接得出，不按时间段推断（见 [roles-and-tasks.md](roles-and-tasks.md) §2.3）。
- 来源：2026-10-01 任务模型讨论。
