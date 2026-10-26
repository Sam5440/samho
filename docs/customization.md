# RPi 自定义配置说明

当前运行时使用 TOML 配置，并拒绝未知字段。环境变量 `RPI_HTTP_BIND`、`RPI_WORKSPACE`、`RPI_DATA_DIR` 分别覆盖对应顶层值。

`workspace`（工作区）和 `data_dir`（数据目录）的相对路径从启动进程工作目录解析；相对 `skill_roots`（技能来源目录）从规范化工作区解析。配置文件所在目录不会自动成为工作区。

## 一、配置入口与生效时间

```powershell
cargo run -p rpi -- config init rpi.toml
cargo run -p rpi -- --config rpi.toml serve
```

`config init` 生成当前版本完整默认配置；为防止覆盖，目标文件已存在时会报错。配置在进程启动时读取；运行时扩展通过 `RuntimeBuilder` 和 Object API 注入。配置文件本身仍需重启服务。

对象层已提供 scope、middleware、generation、lease、排空和失败回滚语义；编辑 TOML 不会自动触发热重载。需要热替换时，应通过受控的 Runtime API 发布新代际，并等待旧代际租约释放。

## 二、模型与路由

`models`（模型列表）可定义适配器、物理模型 ID、端点、API 密钥环境变量、上下文/输出限制、工具/图像/推理能力、推理强度、费用、延迟提示、专业领域和额外请求字段。真实模型示例见 [examples/rpi.chat.toml](../examples/rpi.chat.toml)。

`routing.rules`（路由分类规则）是有序提示词分类器；`tier_penalty`（层级距离惩罚）、`latency_weight`（延迟权重）、`cost_weight`（费用权重）和 `output_reserve_tokens`（输出预留 token）控制选择。任务还可以指定 `complexity`（复杂度）、`tier`（层级）、`model`（模型）、预算、延迟、能力和专业领域。

当前层级是 `nano / fast / balanced / reasoning / frontier / specialist`，数量与复杂度区间固定。`specialist` 按领域约束或显式层级选用，不是高于 `frontier` 的第六档通用智力。费用与延迟提示由使用者提供，并非自动测量事实。预算或延迟过滤掉高候选时，默认评分器可能选择较低通用层级；复杂度不是不可突破的质量下限。

模型 ID 和供应商能力由使用者维护。层级中文名称仅用于界面理解，TOML、命令行和协议里的英文标识必须保持不变。

## 三、工具、MCP、技能与日志

`tools`（工具策略）控制工作区读写、Shell 和 MCP 访问及 Shell 超时。`skill_roots` 控制技能发现来源。`mcp_servers` 接受 stdio 命令或 Streamable HTTP 地址，可通过指定环境变量读取 HTTP Bearer 令牌。`log_content` 控制轨迹载荷详细程度，`durable_logs` 控制强制落盘。

| 领域 | 当前可自定义 | 当前边界 |
| --- | --- | --- |
| 模型 | 多端点/物理 ID、能力、上下文和输出限制、推理强度、价格、延迟提示、领域、额外请求正文 | Provider trait 和自定义 HTTP 头扩展仍需公开版本化插件接口 |
| 路由 | 有序分类、评分权重、输出预留、逐任务模型/层级/能力/预算/延迟覆盖、失败 retry/reroute | 六个内置层级可扩展配置；质量评测仍由部署者提供 |
| 运行时 | 系统提示词、最大轮次、并发任务数、请求超时、工具输出上限、预算与网络策略 | 工具批次默认串行；逐服务并发队列需由扩展实现 |
| 工具 | 读/写/Shell/MCP 策略，通过 `register_tool` 注册 Rust `Object` 工具，支持 schema、hook、取消和动态注册 | 直接对象调用仍是受信任嵌入 API；第三方 ABI/版本隔离需后续扩展 |
| MCP | 服务、stdio 命令/参数/工作目录/环境、环境清理、HTTP 令牌环境变量、请求与启动超时、optional fixture、OAuth PKCE | metadata discovery、动态注册、自动重连和官方 conformance 仍未完成 |
| 技能 | 有序来源目录、YAML 元数据、惰性正文、ignore、资源相对路径、allowed-tools 权限 | 来源热重载和跨进程技能包签名仍待扩展 |
| 日志 | JSONL/SQLite、脱敏、内容开关、span 关系、分页、rotation/retention、timeline/export、SSE | 外部日志后端适配器和大规模分布式索引需后续实现 |
| 界面 | 共用 Runtime 的 Web/TUI、模型/层级/技能选择、进度、fork、steering、trace/Object/MCP 面板 | 主题、快捷键和多工作区导入导出仍待扩展 |
| 会话 | Session/Task Object、checkpoint、恢复、fork、steering/follow-up、上下文 projection 和 lineage | 外部副作用的自动补偿需由具体工具提供幂等语义 |

## 四、扩展自定义 Rust 工具

自定义工具实现 `Object`，使用 `tool:<name>` 格式的 ID、`ObjectKind::Tool` 类型，并在 `spec` 中提供 JSON Schema 对象。`invoke("call", input, context)` 执行工具。

实现应遵守 `context.cancel` 取消信号，验证参数，报告错误，在 `dispose` 中释放资源。`AgentRuntime::register_tool` 安装对象及提供给模型的工具规范。可运行契约示例位于 [agent_integration.rs](../crates/rpi-core/tests/agent_integration.rs) 的 `model_object_and_custom_tool_are_invocable_and_disposed` 测试。当前版本应在接收任务前完成工具注册；generation 热替换通过受控 Runtime API 执行。

直接 `Object` 调用是受信任嵌入 API，尚不会自动获得 Agent 循环全部轨迹埋点；不应当作外部任意调用的开放入口。

## 五、数据记录语义

脱敏器处理配置环境变量秘密及常见凭据模式，不是任意个人数据分类器。`log_content = false` 只省略轨迹正文，不移除会话恢复所需完整消息。`durable_logs = false` 只关闭逐条强制落盘，不关闭文件写入。

数据目录保留脱敏后的会话内容和轨迹，应按对话内容实际性质管理。计划文本填写密钥环境变量名称即可，不要写入密钥值。

## 六、确认下一阶段自定义范围

确认计划页 `/plan` 用于选择下一阶段扩展项，不是运行时配置编辑器。网页勾选后下载 `rpi-确认执行计划-YYYY-MM-DD.txt`，包含选择内容、依赖、顺序和验收要求。只有将完整文本返回当前对话后，才依据它启动后续实现。

GUI 主题、TUI 快捷键、第三方存储/日志后端、分布式索引和可插拔技能来源仍待扩展，其设计入口见[架构说明](architecture.md)和[多场景验收计划](validation-plan.md)。
