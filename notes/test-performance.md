# 测试耗时与构建规模

> 2026-10-09，Lobotomy 云端 Linux 容器实测。不是 Windows 提速数据。

## 改动与覆盖

最终只保留前端测试调度与可选计时报告。没有删除测试，也没有改变测试主体、断言、超时或假 CLI。每个业务场景仍创建自己的临时 host、project 和 Git 仓库。

另外测过将 `lobotomyd` 的 backend、gui、results 三组测试链接为一个二进制。156 个测试的清单不变，共用 helper，仍保留 environment 与 mcp_contract 独立入口。该方案把整个 target 从 2,212,622,315 B 降到 2,025,406,122 B（约 8.5%），三个测试二进制从 249,171,352 B 降到 94,861,592 B。但后端全量运行仅从 37.70 s 变为 36.48 s，冷构建从 432.32 s 变为 481.48 s，无改动增量构建从 0.99 s 变为 6.74 s。运行收益接近波动范围，构建观测更慢，所以撤回该方案。最终没有 Rust 构建或磁盘提速声明，已有调试信息设置也不变。

Playwright 原来用一个 worker 串行运行。现在用 `fullyParallel: true` 和两个 worker，同一文件中的独立场景也可以同时运行。最长的慢机器滚动场景提前声明，避免它在最后单独占用一个 worker，另一个 worker 等待。场景的 CPU 降速、60 秒期限和断言均不变。后端、仓库、端口与 Electron host 的隔离方式不变。Electron 的单实例锁仍共用应用 profile，四个外壳测试显式采用 default 模式，继续在同一个 worker 内顺序执行。首次全并行试跑中一个 Electron 启动失败；限制外壳调度后重跑全量，失败试跑不计入收益表。`--workers=1` 可用于排查。GUI 性能基线的 `bench/bench.config.ts` 仍用一个 worker，避免同时运行的场景干扰性能测量。

`e2e/timing-reporter.cjs` 是可选诊断工具。它记录测试、夹具、API 步骤、断言及其父步骤，不影响默认 reporter。

## 配对测量的条件

基线是 main `b8f59a0275e8a6e23fc3c3b46263832986795b7c`。GUI 优化侧是在这个 SHA 上只改变 Playwright 调度；测试入口合并不改变生产后端，被撤回。后续 main 的 feature 和测试更新另行整合、验证，不混入这组提速数据。

环境：Debian 13.6，x86_64，Xeon Platinum 8573C；CPU 配额为 4 核（`nproc` 显示 5），内存限制 16 GiB。Rust/Cargo 1.99.0，Node 24.19.0，Git 2.52.0，Playwright 1.63.0、Chromium 153.0.8010.12，Electron 44.5.1。编译使用 `CARGO_BUILD_JOBS=4`，仓库已有的 `line-tables-only` 调试信息设置不变。

冷构建指空的 `backend/target`，Cargo registry 已缓存。增量构建指不改源码、不清 target。进程 wall time 用 Python `time.monotonic()` 包住完整命令，包含启动与退出。配对测试按顺序运行，不与编译重叠。两侧生产后端二进制的 SHA-256 完全相同：`dfc0b4752220b2e3ba69b2198163961c54597652e90428b8d81855b59ba4c2db`。Playwright 两次都启动 Vite，使用同一 Xvfb display、默认无头 Chromium，`ELECTRON_DISABLE_SANDBOX=1`，`trace: retain-on-failure`，无重试。

第一轮冷构建的前约半分钟还同时安装、验证前端；第二轮冷构建单独运行。因此冷构建数值只记录观察，不单独归因于本改动。GUI 的配对运行不含这段并发工作。Node、Chromium 和 Electron 的缓存保留，未在每次测试前重建。

## 结果

| 命令或阶段 | 基线 | 仅双 worker，外壳顺序执行 | 双 worker，并提前调度长场景 |
|---|---:|---:|---:|
| 完整 Playwright，外部 wall time | 261.83 s | 216.78 s | 153.70 s |
| 慢机器滚动单个场景 | 28.42 s | 67.60 s | 28.16 s |
| 通过 / 默认跳过 | 16 / 2 | 16 / 2 | 16 / 2 |

最终配置比基线减少约 108.1 s（41.3%）。这是一组共享云端 Linux 数据，未测 Windows 收益。12 个 M1 测试主体逐段比对完全一致，只有声明顺序变化。

失败或中断的试跑不计入表：首次 Electron 全并行启动失败；一次相对路径错误误启动的重复测量已中断。


## 瓶颈拆分

空闲时连续启动 50 次空 Node 程序，启动耗时中位数 208.2 ms、均值 266.4 ms。Git `--version` 中位数 5.9 ms、均值 12.7 ms。20 次创建测试仓库（init、三次 config、写文件、add、commit）的中位数为 51.6 ms。这里测的是完整短命令的启动与退出，不是 `execve` 一个系统调用。

基线 Playwright 的计时报告：16 个通过的测试 duration 合计 234.98 s；测试运行器从开始到结束为 251.93 s，差值 16.94 s 包含 worker/server 启动及测试之间的调度。外部 wall time 为 261.83 s，还包含 npx/Node 启停。

夹具 `backend` 初始化及清理合计 8.14 s。`browser`、`context`、`page` 的夹具步骤合计分别为 1.35、4.47、3.01 s。这些夹具存在嵌套，不能直接相加。没有父级 expect 的断言步骤合计 102.38 s，包含断言重试、等待后端和特意安排的延迟，不是纯比较操作的 CPU 时间。其余时间还有 Electron、浏览器操作、分页与滚动。测试调度和断言等待均有成本，不能把全量耗时都归因于 Node。

诊断跟踪单独运行 `backend` 的 18 个有效场景：成功启动 Git 128 次、Node 21 次、shell 15 次。Git 的 exec 到退出合计 19.44 s，Node 为 24.02 s，但各进程重叠且 Node 包含假 turn、延迟和超时。strace 把该二进制从普通运行的 12.31 s 拖到 53.25 s；这些数值仅用于定位，不用于收益表。没有采用常驻 fake CLI 来换取提速，因为这样会改变对真实进程启动、退出、中断和环境隔离的覆盖。

## 复现

在同一云端机器、相同依赖版本上，分别检出基线与优化测量提交。将 target 移到专用测量目录留存，再运行冷构建；不要删除其他 worker 的目录。每个命令用 Bash 的 `time` 或 monotonic 计时包装，分别保存 stdout、stderr 和退出码：

```bash
cd backend
# 先用空 target，再不改源码重复一次，区分冷构建和增量构建。
time cargo test --workspace --locked --no-run
time cargo test --workspace --locked --no-run
time cargo test --workspace --locked
cargo test --workspace --locked -- --list
du -sb target

cd ../frontend
# Chromium 与 Electron 依赖应预先安装，Xvfb 应预先启动。
export ELECTRON_DISABLE_SANDBOX=1
export DISPLAY=:99
export TEST_TIMING_FILE=/tmp/lobotomy-timing.json
time npx playwright test --reporter=./e2e/timing-reporter.cjs
```

基线代码不带 reporter 时，可将同一 reporter 拷贝到测量目录，使用绝对路径；配对运行必须用相同版本。JSON 中按 category 筛选步骤；只合计没有 expect 父级的 expect，避免 `toPass` 内的断言重复计时。并行时 duration 合计代表各测试工作量，不能当作全套 wall time。

进程诊断与空程序测量另行运行，不与正式 wall-time 测量混跑：

```bash
# 选择基线 cargo test --no-run 输出的 backend 测试二进制。
strace -f -qq -ttt -T -e trace=process -o /tmp/lobotomy-process.trace \
  target/debug/deps/backend-<hash>
# Python subprocess.run 分别执行下面两条命令 50 次，用 monotonic 记录每次启动到退出。
node -e ''
git --version
```

## 验证与限制

最终验证结果待 main 整合后填写。

真实 Codex/Claude 合约测试使用已登录的订阅，仓库默认忽略，本次没有运行。截图审阅场景仅在设置 `SCREENSHOT_DIR` 时运行，本次保持默认跳过。没有用这些局部或未运行项目代替全量默认测试。前端没有单独配置 lint 命令，类型检查、Vitest、生产构建和全量 Playwright 都需要单独运行。

这是一台共享云端 Linux 容器的一组配对数据，磁盘为 apparent size。它不代表 Windows、用户电脑或所有 CI runner 的性能。Windows 和 Ubuntu 的兼容性由现有 CI 的完整测试、Clippy、类型检查、构建及 E2E 验证；CI 的执行时间包含缓存和平台差异，不用它推导本机提速比例。
