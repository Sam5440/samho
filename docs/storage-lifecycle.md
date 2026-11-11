# 增量存储、轨迹查询与对象生命周期

本文对应 session.rs、events.rs、object.rs 和 storage_lifecycle.rs 中已经实现的接口。它不把跨平台 CI 配置、第三方兼容性或尚未执行的压测视为已通过。

## 会话日志：版本 2

SessionStore::open 保留原入口，默认使用 FileJournalBackend。open_with_backend 接受可注入的 JournalBackend；SqliteJournalBackend 使用 SQLite WAL，durable=true 时采用 FULL 同步，false 时采用 NORMAL。文件后端每次追加一条 JSONL。

每个增量记录包含 schema_version、全局 sequence、previous_checksum、metadata、truncate_to、append_messages 和 checksum。checksum 是规范化 Rust 序列化记录的 SHA-256，构成前后关联的校验链。新增消息只追加尾部；修改或压缩已有消息时，从共同前缀后截断并写入替代尾部，不再为每次提交写完整 transcript。

当前提交仍需在内存中比较共同前缀，且为脱敏与恢复投影保留完整会话。磁盘追加体积已按增量增长，提交 CPU/内存暂未声称为常数复杂度。

读取旧的完整 Session 快照时，先验证全部完整记录，再转成版本 2。文件后端保留 .legacy.jsonl 副本，候选迁移文件写完后再重命名发布；SQLite 替换在一个事务中完成。校验失败的完整内部记录拒绝打开，原文件不被修补。只有经过内部验证后，未换行的坏尾才被截断。未知版本、错序和 SHA 链不一致会报错。

JournalBackend 公共契约要求 append 与 replace 原子成功或报错；自定义实现不能把部分写入报告为成功。失败不推进 SessionStore 的提交序号。文件和 SQLite 的后端故障机制并不等同操作系统隔离。

## 分支与恢复

fork(parent_id, cutoff, title) 创建新会话，记录 parent_id 与 fork_cutoff，保留父会话。cutoff 是消息数量，至少保留系统消息；不能切断未完成的工具调用组，也不能在父会话 running 时创建分支。

分支初始记录引用已提交父投影的截止点，后续消息属于分支自身。父会话后续追加不改变已创建分支。重启后的 running 会话投影为 interrupted；存储加载、迁移和分支不会运行工具，外部副作用不会自动重试。

## 轨迹查询与保留

EventLog::open、events、export_jsonl 保留兼容入口。open_with_options 接受 rotation_bytes 和 max_archives；rotation_bytes=0 关闭轮转，启用轮转时至少保留一个归档。超过阈值后，当前文件转成按序号命名的 segment 文件；历史归档达到上限后删除最早归档。单条过大记录允许独占一个片段，而不会被拆开。

文件后端启动时重建按事件序号排列的字节偏移索引。EventQuery 的 after_sequence 为排他的序号游标，limit 为 1–10000，支持 trace_id/session_id/kind 过滤。EventPage 返回 events、next_cursor、has_more 和 first_available_sequence。后者用于识别保留策略导致的历史缺口。

查询只复制匹配的索引条目，随后读取对应字节范围；读取和 JSON 解码不持有追加锁。索引保留元数据而非完整 payload，仍占随事件数量增长的内存。过滤目前扫描候选索引元数据，尚不是全部字段的复合磁盘索引。

open_with_backend 可使用 SQLite 或自定义 JournalBackend。此路径按日志行顺序恢复和查询，当前会读取后端记录集；它没有宣称 SQLite 查询条件已映射为数据库索引。此路径要求 rotation_bytes=0，后端可自行实现存储保留；文件索引与数据库后端性能应分别评测。

timeline(trace_id) 按 span 聚合序号、父 span、Object、起止时间、事件数量和类型。duration_ms 是墙钟时间的观察区间，不是跨进程单调时钟性能测量。schema_metadata 说明版本、游标语义、当前后端和保留范围。

## 对象租约与代际

ObjectRegistry 保留 register/get/list。get 是兼容的受信任原始访问，无法追踪持有者；托管执行必须走 acquire 或 invoke_registered。

ObjectLease 固定 ObjectIdentity 的 ID、generation 和 scope，释放租约后减少在途计数。invoke_registered 在租约内执行 middleware 的 before 和逆序 after，再释放租约。before 可校验/变换参数并拒绝执行，after 可观察结果；它不是可绕过上层工具权限的授权机制。

hot_replace 先私下激活候选，再阻止旧代际取得新租约并等待排空。候选激活失败或排空超时会释放候选并恢复旧代际。成功时发布新代际后释放旧对象。旧对象 dispose 报错时，新代际已经生效，错误会明确说明此边界；异步原生插件的资源释放无法通用地回滚。

为避免依赖对象持有无法追踪的原生引用，当前热替换拒绝有已注册依赖者的对象，并要求新对象保留类型和依赖边。这是明确的保守限制，不是任意依赖图的事务式整图热替换。普通 activate 的失败对象应自行清理 activate 中的部分资源；注册表逆序释放本次已成功激活的依赖，保留旧公共契约。

register_scope/register_in_scope 提供根与子作用域，依赖只能引用可见祖先或同作用域对象，禁止兄弟作用域越界。dispose_scope 按逆序排空并释放所属活跃对象；dispose_with_timeout 同样等待租约。作用域可见性是框架契约，不是进程或文件系统沙箱。

## 验证证据

执行：

~~~powershell
cargo test -p samho-core --test storage_lifecycle
cargo test -p samho-core --test core_contracts
~~~

新增 9 项存储/生命周期测试已通过：文件/SQLite 一致性、旧格式迁移、内部损坏/坏尾、fork 边界、ENOSPC 故障注入、轨迹轮转保留/分页恢复、SQLite 事件查询、租约超时及失败回滚、作用域与 middleware，以及跨分块凭据脱敏。既有 15 项 core_contracts 同样通过。全工作区与真实平台 CI 仍需按最终集成版本重新执行。
