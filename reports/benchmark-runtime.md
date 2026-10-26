# 本机发布构建模拟任务基准

2026-10-07，Windows 11 / AMD Ryzen AI 9 H 365，Rust 1.96.0 发布构建（release）。执行 `scripts/benchmark-runtime.ps1`：3 次预热、20 次全新进程，使用独立工作区和数据目录，`durable_logs=true`，禁用文件、Shell 和 MCP 工具，技能（Skill）目录为空，显式选择一个模拟（Demo）模型。内置 `load_skill` 工具规范仍会发布，但本任务未调用工具，不访问网络或收费模型。

| 指标 | 最小值 | 第 50 百分位（p50） | 第 95 百分位（p95） | 最大值 |
| --- | ---: | ---: | ---: | ---: |
| 进程启动至退出（毫秒） | 84.553 | 88.946 | 103.658 | 105.081 |
| Windows 峰值工作集（字节） | 9,719,808 | 9,777,152 | 9,797,632 | 9,859,072 |

耗时覆盖进程启动、运行时初始化、路由、模拟任务、日志持久化及关闭，并未分别测量这些阶段；预热后操作系统缓存可能已经命中。内存来自 Windows `GetProcessMemoryInfo` 的 `PeakWorkingSetSize`（峰值工作集）字段，20 个样本全部读取成功。百分位按最近秩法（nearest-rank）计算。

全部原始样本、二进制 SHA-256、编译器/Node/硬件信息与上游 Pi 提交 ID 保存在 [`benchmark-runtime.json`](benchmark-runtime.json)。主工程 Git 已初始化但尚无提交，因此工作区提交字段标为 `unavailable`（不可用）；二进制 SHA-256 用于标识本次测量版本。

这组结果只描述本机模拟任务的运行开销，不能推导相对 Pi 的性能倍率、真实模型响应速度或 Agent 任务质量。
