/*
 * RPi 确认计划编排器
 *
 * 这是一个只负责“编排和导出确认计划”的离线模块。它不会调用 /api/run，
 * 也不会启动任务。页面或宿主工作台可以通过 window.RpiPlan.mount(root)
 * 挂载它，纯文本生成与依赖校验函数也会暴露给测试脚本。
 */
(function (global) {
  'use strict';

  var STATUS = {
    done: '已完成（首版已验证）',
    partial: '部分完成（有明确边界）',
    planned: '未完成（确认后实施）'
  };

  var GROUPS = [
    {
      id: 'phase',
      title: '功能阶段',
      description: '决定这份计划要覆盖的主要交付阶段。',
      items: [
        { id: 'upstream-analysis', label: '上游 Pi / DSH / ZCode 源码分析', status: 'done', detail: '固定提交、功能矩阵、任务循环与性能边界' },
        { id: 'rust-workspace', label: 'Rust workspace 与项目骨架', status: 'done', detail: 'rpi-core、rpi-cli、配置与本地数据目录' },
        { id: 'object-lifecycle', label: 'Everything is Object 生命周期', status: 'done', detail: '依赖拓扑、激活回滚、逆序销毁、动态 Tool' },
        { id: 'agent-loop', label: 'Agent 任务循环与会话恢复', status: 'done', detail: '取消、并发限制、检查点、工具调用循环' },
        { id: 'provider-streaming', label: 'Provider 流式响应与结构化错误', status: 'planned', detail: '增量文本 / 工具调用、取消、背压和 typed error' },
        { id: 'context-compaction', label: '上下文投影与可追溯压缩', status: 'planned', detail: 'Token 计数、压缩策略、摘要血缘和上下文保护' },
        { id: 'session-tree', label: '会话树、fork 与任务 steering', status: 'planned', detail: '分支、追问、执行中引导与副作用恢复边界' },
        { id: 'retry-fallback', label: '重试、降级、升级与预算封顶', status: 'planned', detail: '任务内路由、可重试错误、幂等和消耗追踪' },
        { id: 'model-routing', label: '模型分级与自动路由', status: 'done', detail: '六级策略、能力 / 预算 / 延迟 / 上下文过滤' },
        { id: 'provider-layer', label: 'Provider 适配与真实模型接入', status: 'partial', detail: 'Demo 模拟器与 Chat Completions 兼容 Provider 已有；流式仍待迁移' },
        { id: 'trace-storage', label: '日志、事件轨迹与复盘存储', status: 'done', detail: '脱敏 JSONL、span 关系、序列号、重启恢复、SSE' },
        { id: 'mcp-support', label: 'MCP 客户端与 Tool 注册', status: 'partial', detail: 'stdio / Streamable HTTP 已验证；可选协议能力仍有边界' },
        { id: 'skill-support', label: 'Skill 发现、解析与惰性加载', status: 'partial', detail: 'SKILL.md 元数据和惰性加载已验证；资源策略待扩展' },
        { id: 'tui', label: '共用 Runtime 的 TUI', status: 'done', detail: 'Ratatui 会话、提交、取消和轨迹查看' },
        { id: 'web-gui', label: '本地 Web GUI', status: 'done', detail: '工作台、对象 / MCP / Skill / Trace 面板和 SSE' },
        { id: 'validation', label: '多场景、多任务完整测试', status: 'partial', detail: 'Rust 契约与集成测试已通过；上游全量和真实模型仍需单独验收' },
        { id: 'release', label: '发布、部署与跨平台验收', status: 'partial', detail: 'Windows 本地 release smoke 已有；跨平台和生产运维仍待实施' }
      ]
    },
    {
      id: 'model',
      title: '模型分级',
      description: '选择确认计划中要固定或验收的智力层级。层级是策略标签，不代表统一基准排名。',
      items: [
        { id: 'nano', label: 'Nano · 轻量分类 / 格式整理', status: 'done', detail: '低预算、低延迟策略' },
        { id: 'fast', label: 'Fast · 常规问答 / 快速修改', status: 'done', detail: '优先响应时间' },
        { id: 'balanced', label: 'Balanced · 均衡任务', status: 'done', detail: '质量、成本和延迟平衡' },
        { id: 'reasoning', label: 'Reasoning · 多步推理 / 调试', status: 'done', detail: '要求 reasoning 能力' },
        { id: 'frontier', label: 'Frontier · 高难综合任务', status: 'done', detail: '更高上下文和预算上限' },
        { id: 'specialist', label: 'Specialist · 专项能力', status: 'done', detail: '按 specialty 能力匹配' }
      ]
    },
    {
      id: 'protocol',
      title: 'MCP 与 Skill',
      description: '记录需要纳入实施与验收的扩展边界。',
      items: [
        { id: 'mcp-stdio-http', label: 'MCP stdio 与 Streamable HTTP', status: 'done', detail: '初始化、工具发现、调用、超时和关闭' },
        { id: 'mcp-resources-prompts', label: 'MCP 资源 / 提示', status: 'done', detail: 'resources / prompts：列表、分页、读取和参数传递' },
        { id: 'mcp-optional', label: 'MCP OAuth / sampling / elicitation / tasks 等可选能力', status: 'planned', detail: '当前明确未实现，需能力协商与真实 fixture' },
        { id: 'skill-metadata', label: 'Skill SKILL.md 元数据与惰性加载', status: 'done', detail: '重复检测、元数据立即可见' },
        { id: 'skill-policy', label: 'Skill ignore / resources / allowed-tools 策略', status: 'planned', detail: '当前仍为后续阶段' }
      ]
    },
    {
      id: 'observability',
      title: '日志与调用轨迹',
      description: '选择复盘、审计和运维所需的记录粒度。默认保护敏感内容。',
      items: [
        { id: 'trace-spans', label: 'trace / span / parent span 关联', status: 'done', detail: '任务、路由、模型和工具调用可串联' },
        { id: 'redaction', label: '凭据脱敏与内容开关', status: 'done', detail: '默认不记录完整内容；显式开启后仍脱敏' },
        { id: 'trace-index', label: '轨迹索引、分页、保留与导出策略', status: 'planned', detail: '当前事件历史为 JSONL 扫描' },
        { id: 'replay', label: '可复盘报告与故障注入', status: 'partial', detail: '当前支持记录查看和重启恢复，重放语义仍有限' }
      ]
    },
    {
      id: 'interface',
      title: 'TUI / GUI 交付形态',
      description: '确认用户将如何使用同一个 Rust 服务。',
      items: [
        { id: 'shared-runtime', label: 'Web GUI 与 TUI 共用 Rust Runtime', status: 'done', detail: '同一会话、对象、模型策略和事件流' },
        { id: 'web-local', label: '本地 loopback Web 工作台', status: 'done', detail: '默认仅绑定本机回环地址' },
        { id: 'tui-terminal', label: '终端会话、取消与轨迹查看', status: 'done', detail: 'Ratatui 客户端' },
        { id: 'ui-themes', label: '主题、快捷键和多工作区定制', status: 'planned', detail: '配置化 UI 仍待扩展' }
      ]
    },
    {
      id: 'quality',
      title: '测试与性能',
      description: '选择执行门槛和验证证据。性能数字必须来自同机、同任务的实测。',
      items: [
        { id: 'contract-tests', label: 'Rust 契约、Agent、MCP 集成测试', status: 'done', detail: '当前 workspace 37 项通过' },
        { id: 'upstream-tests', label: 'Pi 上游分包回归测试', status: 'partial', detail: '选择性测试通过；Windows TUI 和全量 npm 环境差异已记录' },
        { id: 'scenario-matrix', label: '多场景、多任务验收矩阵', status: 'partial', detail: '基础矩阵已定义，真实 Provider 与长会话仍需补齐' },
        { id: 'performance-benchmark', label: '同机性能基准与长会话压测', status: 'partial', detail: '已有本机模拟器样本；不能推导相对 Pi 的倍率' },
        { id: 'fault-injection', label: '取消、超时、崩溃恢复与故障注入', status: 'planned', detail: '需扩展持久化和外部副作用测试' }
      ]
    }
  ];

  GROUPS.push({
    id: 'customization',
    title: '自定义与扩展范围',
    description: '确定哪些层必须提供公开、稳定、可测试的替换接口。',
    items: [
      { id: 'custom-provider', label: '自定义 Provider / Router', status: 'planned', detail: '可替换 trait、Provider 能力、路由规则与动态层级' },
      { id: 'custom-tool', label: '自定义 Tool / Object', status: 'partial', detail: '动态 Tool 已实现；统一 hook / 中间件需补齐' },
      { id: 'custom-storage', label: '自定义存储与日志后端', status: 'planned', detail: '增量会话、SQLite / 文件存储、事件索引和保留策略' },
      { id: 'hot-plugins', label: '插件 generation / scope / lease 与热替换', status: 'planned', detail: '排空、回滚、资源租约、热重载和兼容版本' },
      { id: 'custom-policy', label: '权限、预算、工具与网络策略', status: 'partial', detail: '现有路径约束和写入 / shell opt-in；可插拔授权待扩展' },
      { id: 'remote-facets', label: '远程 Facet / 多进程协作', status: 'planned', detail: '协议边界、进程隔离、取消传播和分布式轨迹' }
    ]
  });

  var ACCEPTANCE = {
    'upstream-analysis': '保持固定 commit、功能差距表、源码引用与许可证来源；上游变化可追踪。',
    'rust-workspace': 'cargo fmt --check、cargo test --workspace 和 clippy -D warnings 通过；release 可构建。',
    'object-lifecycle': '缺失依赖、循环依赖、激活失败回滚、逆序销毁与动态注册 fixture 通过。',
    'agent-loop': '提交、工具循环、取消、并发限制、重启恢复和 turn / budget 边界通过。',
    'provider-streaming': '断流、增量工具参数、取消、背压和网络错误均有协议 fixture；GUI / TUI 同步展示。',
    'context-compaction': '长会话在可控容量内完成；保留系统指令与必要工具观察；摘要可关联源消息。',
    'session-tree': 'fork 不覆盖父分支；steering / follow-up 顺序确定；恢复不自动重复外部副作用。',
    'retry-fallback': '可重试 / 不可重试错误分类、最大次数、累计预算封顶和路由理由有测试与轨迹。',
    'model-routing': '覆盖每级正例 / 拒绝例、能力 / 上下文 / 延迟 / 预算过滤、显式模型覆盖与理由。',
    'provider-layer': '模拟器输出明确标记；真实 Provider 用独立凭据、录制协议和受控任务验收。',
    'trace-storage': '序列号单调、span 关系完整、SSE gap 恢复、尾记录恢复和凭据脱敏通过。',
    'mcp-support': '发布可追踪的协议 / SDK 版本及方法矩阵；未支持方法显式报错。',
    'skill-support': '解析合法 / 非法 YAML、重名、惰性加载和显式加载事件可复现。',
    'tui': '会话切换、输入、取消、窗口缩放和 Unicode 宽度验收；与 Web 状态一致。',
    'web-gui': '任务提交、失败 / 取消 / 完成、Trace / Object / MCP / Skill 查看和重启 smoke 通过。',
    'validation': '场景矩阵每项给出通过 / 失败 / 跳过和原始证据；不把计划门槛写作通过。',
    'release': '跨平台构建、启动、退出、配置迁移与可重现安装流程验收。',
    'mcp-stdio-http': '真实 stdio / HTTP fixture 覆盖初始化、请求关联、超时、取消和子进程回收。',
    'mcp-resources-prompts': '资源 / 模板 / prompts 分页、读取、参数及能力声明拒绝行为通过。',
    'mcp-optional': '逐项实现 OAuth、sampling、roots、elicitation、tasks、subscriptions、completion 与 Schema 校验；公布未覆盖项。',
    'skill-metadata': '元数据验证、重复检测和正文惰性加载行为一致。',
    'skill-policy': 'ignore、资源相对路径、来源重载与 allowed-tools 权限按 fixture 强制执行。',
    'trace-spans': '任务、路由、Provider 和工具事件按 trace / parent span 串联，可查询。',
    'redaction': '凭据、个人敏感数据和内容开关负向测试通过；日志中不残留测试秘密。',
    'trace-index': '支持游标分页、索引恢复、保留 / rotation、导出和大日志并发追加；查询不长期阻塞写入。',
    'replay': '提供中文事件时间线、路由理由、错误定位和导出；展示式复盘与重新执行严格区分。',
    'shared-runtime': 'TUI / Web 同服务共享会话、任务、对象与事件；不产生两套独立执行循环。',
    'web-local': '回环绑定、同源限制和 CSP 验证；页面可展示实际运行状态。',
    'tui-terminal': '终端按键、取消和轨迹查看人工 / 自动验收记录齐全。',
    'ui-themes': '主题 / 快捷键 / 工作区配置可导入导出；中文可读性和键盘可达性通过。',
    'contract-tests': '保留现有 37 项基线并补充变更引入的行为测试；无回归。',
    'upstream-tests': '隔离运行相关 Pi 包测试；记录 Windows / POSIX 差异与 npm PATH 环境失败。',
    'scenario-matrix': '覆盖研究、代码修改、多步工具、长会话、MCP / Skill 组合、取消和多任务并发。',
    'performance-benchmark': '同机等价 Pi / Rust 原始样本、p50 / p95、RSS、吞吐和长会话增长曲线齐全。',
    'fault-injection': '覆盖网络断连、磁盘失败、Provider 错误、进程崩溃、恢复和副作用幂等。',
    'custom-provider': '第三方实现可不修改核心而注入；接口版本、失败隔离和配置 schema 有例子。',
    'custom-tool': '输入 / 输出 schema、生命周期、hook、错误、取消和动态注册文档齐全。',
    'custom-storage': '更换后端保留会话 / 轨迹语义；增量 journal 避免全快照二次增长；迁移与恢复可测。',
    'hot-plugins': '排空旧 generation、租约释放、失败回滚和并发热替换 fixture 通过。',
    'custom-policy': '规则明确、可配置、有拒绝原因；写入 / shell / 网络的边界被测试强制执行。',
    'remote-facets': '进程隔离、断连、取消、版本协商、远程认证与 trace 传播可验收。'
  };

  var RECOMMENDED = ['upstream-analysis', 'rust-workspace', 'object-lifecycle', 'agent-loop', 'provider-streaming', 'context-compaction', 'retry-fallback', 'model-routing', 'provider-layer', 'trace-storage', 'mcp-support', 'skill-support', 'tui', 'web-gui', 'validation', 'nano', 'fast', 'balanced', 'reasoning', 'frontier', 'specialist', 'mcp-stdio-http', 'mcp-resources-prompts', 'skill-metadata', 'trace-spans', 'redaction', 'shared-runtime', 'web-local', 'tui-terminal', 'contract-tests', 'scenario-matrix', 'custom-tool', 'custom-policy'];

  var CURRENT_STATUS = [
    { label: '上游源码下载与静态分析', status: 'done', detail: 'Pi、DSH、ZCode 已固定提交并形成中文分析文档。' },
    { label: 'Rust Object / Agent / 路由 / Provider 首版', status: 'done', detail: 'Object 生命周期、任务循环、六级模型策略和 Demo / Chat Completions Provider 已实现。' },
    { label: '日志、轨迹、TUI 与本地 Web GUI', status: 'done', detail: 'JSONL 脱敏轨迹、SSE、Ratatui 和本地工作台已通过首版验收。' },
    { label: 'MCP 与 Skill', status: 'partial', detail: '已验证协议子集与 SKILL.md 加载；完整可选能力和资源策略仍未完成。' },
    { label: 'Provider 流式、上下文压缩、会话树 / fork', status: 'planned', detail: '尚未迁移，不能在确认计划中表述为已完成。' },
    { label: '插件热替换、远程 Facet、跨平台生产运维', status: 'planned', detail: '需要 generation / lease、跨平台回归和长期 soak。' }
  ];

  var DEPENDENCIES = [
    { when: ['web-gui'], needs: ['shared-runtime'], message: 'Web GUI 依赖共用 Rust Runtime。' },
    { when: ['tui'], needs: ['agent-loop'], message: 'TUI 需要 Agent 任务循环与会话接口。' },
    { when: ['mcp-stdio-http', 'mcp-resources-prompts', 'mcp-optional'], needs: ['mcp-support'], message: 'MCP 细项依赖 MCP 客户端阶段。' },
    { when: ['skill-metadata', 'skill-policy'], needs: ['skill-support'], message: 'Skill 细项依赖 Skill 发现与加载阶段。' },
    { when: ['reasoning', 'frontier', 'specialist'], needs: ['model-routing'], message: '模型层级验收依赖模型路由阶段。' },
    { when: ['trace-index', 'replay', 'fault-injection'], needs: ['trace-storage'], message: '轨迹扩展依赖基础日志与事件存储。' },
    { when: ['provider-streaming', 'retry-fallback', 'custom-provider'], needs: ['provider-layer'], message: 'Provider 扩展依赖既有 Provider 适配阶段。' },
    { when: ['context-compaction', 'session-tree'], needs: ['agent-loop'], message: '上下文与会话语义依赖既有 Agent 循环。' },
    { when: ['hot-plugins', 'remote-facets', 'custom-tool'], needs: ['object-lifecycle'], message: '插件与自定义工具依赖 Object 生命周期。' }
  ];

  function allItems() {
    var result = [];
    GROUPS.forEach(function (group) {
      group.items.forEach(function (item) {
        result.push({ group: group, item: item });
      });
    });
    return result;
  }

  function itemMap() {
    var map = Object.create(null);
    allItems().forEach(function (entry) { map[entry.item.id] = entry.item; });
    return map;
  }

  function today(value) {
    if (value) {
      var match = String(value).match(/^(\d{4})-(\d{2})-(\d{2})$/);
      if (match) return String(value);
    }
    var now = new Date();
    return now.getFullYear() + '-' + String(now.getMonth() + 1).padStart(2, '0') + '-' + String(now.getDate()).padStart(2, '0');
  }

  function listText(values) {
    return values && values.length ? values.join('、') : '未选择';
  }

  function selectedEntries(data) {
    var selected = data && data.selected ? data.selected : {};
    return allItems().filter(function (entry) { return Boolean(selected[entry.item.id]); });
  }

  function computeDependencies(data) {
    var selected = data && data.selected ? data.selected : {};
    var warnings = [];
    DEPENDENCIES.forEach(function (rule) {
      var triggered = rule.when.some(function (id) { return Boolean(selected[id]); });
      if (!triggered) return;
      var missing = rule.needs.filter(function (id) { return !selected[id]; });
      if (missing.length) warnings.push({ message: rule.message, missing: missing });
    });
    return warnings;
  }

  function validatePlan(data) {
    var errors = [];
    if (!data || !String(data.target || '').trim()) errors.push('请填写项目目标，便于执行者判断完成标准。');
    if (!data || selectedEntries(data).length === 0) errors.push('至少勾选一个执行范围。');
    if (!data || !String(data.priority || '').trim()) errors.push('请选择最高优先级。');
    if (data && data.selected && typeof data.selected === 'object') {
      var known = itemMap();
      var unknown = Object.keys(data.selected).filter(function (id) { return data.selected[id] && !known[id]; });
      if (unknown.length) errors.push('包含未知执行范围：' + unknown.join('、') + '。请刷新计划页后重新选择。');
    }
    return { valid: errors.length === 0, errors: errors, warnings: computeDependencies(data || {}) };
  }

  function statusLine(entry) {
    var item = entry.item || entry;
    var status = STATUS[item.status] || item.status || STATUS.planned;
    return '【' + status + '】' + item.label + '：' + (item.detail || '');
  }

  function generatePlanText(data, options) {
    data = data || {};
    options = options || {};
    var date = today(options.date || data.date);
    var selected = selectedEntries(data);
    var byGroup = Object.create(null);
    selected.forEach(function (entry) {
      if (!byGroup[entry.group.id]) byGroup[entry.group.id] = [];
      byGroup[entry.group.id].push(entry.item);
    });
    var lines = [];
    lines.push('RPi Rust Agent 重写 · 确认执行计划');
    lines.push('='.repeat(44));
    lines.push('生成日期：' + date);
    lines.push('计划状态：待确认；本文件只描述执行计划，不会自动执行任务。');
    lines.push('计划文件：rpi-确认执行计划-' + date + '.txt');
    lines.push('');
    lines.push('一、项目目标');
    lines.push('项目名称：' + (String(data.projectName || '').trim() || 'RPi Rust Agent 重写')); 
    lines.push('目标描述：' + (String(data.target || '').trim() || '未填写')); 
    lines.push('');
    lines.push('二、优先级与执行边界');
    lines.push('最高优先级：' + (String(data.priority || '').trim() || '未选择'));
    lines.push('执行边界：' + (String(data.boundary || '').trim() || '未填写；按仓库现有边界执行。'));
    lines.push('使用形态：' + (String(data.mode || '').trim() || '本地 Web GUI 与 TUI 共用 Rust 服务'));
    lines.push('实施排序：' + (String(data.sequence || '').trim() || '按本计划的阶段依赖依次交付。'));
    lines.push('目标平台：' + listText(data.platforms));
    lines.push('模型与供应商偏好：' + (String(data.models || '').trim() || '供应商 / 模型 ID 不预绑定；通过外部配置指定。'));
    lines.push('模型预算边界：' + (String(data.budget || '').trim() || '未指定数额；在真实付费调用前必须明确配置预算。'));
    lines.push('调用轨迹内容策略：' + (data.logContent ? '允许配置完整调用轨迹内容记录；必须保留脱敏、访问与保留策略。' : '调用轨迹默认只记录元数据与脱敏事件，不记录完整 Prompt / 响应；会话持久化仍保留完整脱敏消息，需另行管理。'));
    lines.push('');
    lines.push('三、已勾选的实施范围');
    GROUPS.forEach(function (group) {
      var items = byGroup[group.id] || [];
      if (!items.length) return;
      lines.push('[' + group.title + ']');
      items.forEach(function (item) {
        lines.push('• ' + item.label + '（' + (STATUS[item.status] || item.status) + '）');
        lines.push('  工作内容：' + item.detail + '。');
        lines.push('  验收门槛：' + (ACCEPTANCE[item.id] || '按所选模型层级配置能力、上下文、预算、延迟与 specialty；覆盖选中 / 拒绝用例，不将策略层级等同于模型基准排名。'));
      });
    });
    if (!selected.length) lines.push('• 未选择');
    var unselected = allItems().filter(function (entry) { return !data.selected || !data.selected[entry.item.id]; });
    lines.push('');
    lines.push('未勾选范围（本轮不执行，保留为后续候选）');
    if (unselected.length) {
      unselected.forEach(function (entry) { lines.push('• ' + entry.item.label + '（' + (STATUS[entry.item.status] || entry.item.status) + '）'); });
    } else {
      lines.push('• 无；已勾选全部定义范围。');
    }
    lines.push('');
    lines.push('四、当前基线（必须如实保留）');
    CURRENT_STATUS.forEach(function (entry) { lines.push('• ' + statusLine(entry)); });
    lines.push('');
    lines.push('五、依赖与风险提示');
    var validation = validatePlan(data);
    if (validation.warnings.length) {
      validation.warnings.forEach(function (warning) { lines.push('• ' + warning.message + ' 缺少：' + warning.missing.join('、') + '。'); });
    } else {
      lines.push('• 未发现已勾选范围之间的硬依赖缺口。');
    }
    lines.push('• Demo Provider 明确标记为模拟输出，真实模型需单独配置并验收。');
    lines.push('• 当前 MCP 仅为已验证方法子集，不宣称完整 MCP 规范兼容。');
    lines.push('• 性能目标必须通过同机、同任务、同配置基准校准，不直接宣称相对 Pi 的固定倍率。');
    lines.push('• 硬依赖未满足时先复核范围；验收失败保留证据并停止进入下一阶段。');
    lines.push('• 迁移保留可恢复检查点，不盲目重跑具有外部副作用的调用。');
    lines.push('');
    lines.push('六、验证矩阵与交付物');
    lines.push('场景范围：' + listText(data.scenarios));
    lines.push('验证强度：' + (String(data.testLevel || '').trim() || '契约 + 集成 + GUI / TUI smoke'));
    lines.push('交付要求：' + (String(data.deliverables || '').trim() || 'Rust 源码、中文架构 / 使用 / 能力文档、原始测试结果、调用轨迹与性能报告。'));
    lines.push('执行操作约束：' + (String(data.operations || '').trim() || '仅在当前仓库实施与本地验证；无自动部署、推送、外部消息或付费调用。'));
    lines.push('');
    lines.push('七、确认后的建议执行顺序');
    lines.push('1. 锁定目标、边界、优先级以及本计划文件版本。');
    lines.push('2. 对“未完成”和“部分完成”项逐项补齐实现、测试和证据。');
    lines.push('3. 运行 Rust workspace、上游选择性测试、MCP / Skill fixture 及浏览器 / TUI 验收。');
    lines.push('4. 生成日志、调用轨迹、测试报告和性能原始样本；失败项保留复盘入口。');
    lines.push('5. 每阶段更新中文状态矩阵与证据，未通过项说明原因、影响与后续措施。');
    lines.push('');
    lines.push('八、回传确认模板');
    lines.push('我确认执行《RPi Rust Agent 重写 · 确认执行计划》（' + date + '）。');
    lines.push('执行范围：' + (selected.length ? selected.map(function (entry) { return entry.item.label; }).join('、') : '未选择'));
    lines.push('最高优先级：' + (String(data.priority || '').trim() || '未选择')); 
    lines.push('补充说明：' + (String(data.notes || '').trim() || '无')); 
    lines.push('请按上述范围、约束、顺序和验收门槛在当前仓库执行；尚未完成的能力不得表述为已完成。');
    lines.push('');
    lines.push('—— 文档结束 ——');
    return lines.join('\n');
  }

  function filename(data, options) {
    options = options || {};
    return 'rpi-确认执行计划-' + today(options.date || data && data.date) + '.txt';
  }

  function byId(root, id) {
    return root && root.querySelector ? root.querySelector('#' + id) : null;
  }

  function checked(root, name) {
    return Array.prototype.slice.call(root.querySelectorAll('input[data-plan-item][data-plan-group="' + name + '"]:checked')).map(function (input) { return input.value; });
  }

  function collect(root) {
    var selected = {};
    Array.prototype.slice.call(root.querySelectorAll('input[data-plan-item]')).forEach(function (input) { selected[input.value] = Boolean(input.checked); });
    return {
      projectName: byId(root, 'plan-project-name') ? byId(root, 'plan-project-name').value : '',
      target: byId(root, 'plan-target') ? byId(root, 'plan-target').value : '',
      priority: byId(root, 'plan-priority') ? byId(root, 'plan-priority').value : '',
      boundary: byId(root, 'plan-boundary') ? byId(root, 'plan-boundary').value : '',
      mode: byId(root, 'plan-mode') ? byId(root, 'plan-mode').value : '',
      notes: byId(root, 'plan-notes') ? byId(root, 'plan-notes').value : '',
      sequence: byId(root, 'plan-sequence') ? byId(root, 'plan-sequence').value : '',
      models: byId(root, 'plan-models') ? byId(root, 'plan-models').value : '',
      budget: byId(root, 'plan-budget') ? byId(root, 'plan-budget').value : '',
      testLevel: byId(root, 'plan-test-level') ? byId(root, 'plan-test-level').value : '',
      deliverables: byId(root, 'plan-deliverables') ? byId(root, 'plan-deliverables').value : '',
      operations: byId(root, 'plan-operations') ? byId(root, 'plan-operations').value : '',
      logContent: byId(root, 'plan-log-content') ? byId(root, 'plan-log-content').checked : false,
      platforms: Array.prototype.slice.call(root.querySelectorAll('[data-plan-platform]:checked')).map(function (input) { return input.value; }),
      scenarios: Array.prototype.slice.call(root.querySelectorAll('[data-plan-scenario]:checked')).map(function (input) { return input.value; }),
      date: byId(root, 'plan-date') ? byId(root, 'plan-date').value : today(),
      selected: selected,
      selectedByGroup: GROUPS.reduce(function (result, group) { result[group.id] = checked(root, group.id); return result; }, {})
    };
  }

  function setAll(root, mode) {
    Array.prototype.slice.call(root.querySelectorAll('input[data-plan-item]')).forEach(function (input) {
      if (mode === 'all') input.checked = true;
      else if (mode === 'none') input.checked = false;
      else if (mode === 'recommended') input.checked = RECOMMENDED.indexOf(input.value) !== -1;
    });
  }

  function element(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = String(text);
    return node;
  }

  function renderGroups(root) {
    var host = byId(root, 'plan-groups');
    if (!host) return;
    host.replaceChildren();
    GROUPS.forEach(function (group) {
      var section = element('section', 'plan-group');
      section.dataset.planGroup = group.id;
      var heading = element('div', 'plan-group-heading');
      var headingCopy = element('div');
      headingCopy.append(element('span', 'plan-kicker', group.id.toUpperCase()), element('h3', '', group.title), element('p', '', group.description));
      var count = element('span', 'plan-group-count', '0 项');
      count.dataset.planCount = group.id;
      heading.append(headingCopy, count);
      section.appendChild(heading);
      var grid = element('div', 'plan-check-grid');
      group.items.forEach(function (item) {
        var label = element('label', 'plan-check plan-status-' + item.status);
        var input = element('input');
        input.type = 'checkbox';
        input.dataset.planItem = '';
        input.dataset.planGroup = group.id;
        input.value = item.id;
        input.checked = true;
        var box = element('span', 'plan-box');
        box.setAttribute('aria-hidden', 'true');
        var copy = element('span', 'plan-check-copy');
        copy.append(element('strong', '', item.label), element('small', '', item.detail));
        label.append(input, box, copy, element('em', '', STATUS[item.status] || item.status));
        grid.appendChild(label);
      });
      section.appendChild(grid);
      host.appendChild(section);
    });
  }

  function renderCurrentStatus(root) {
    var host = byId(root, 'plan-current-status');
    if (!host) return;
    host.replaceChildren();
    CURRENT_STATUS.forEach(function (entry) {
      var item = element('li', 'plan-status-row plan-status-row-' + entry.status);
      var pip = element('span', 'plan-status-pip');
      pip.setAttribute('aria-hidden', 'true');
      var copy = element('div');
      copy.append(element('strong', '', entry.label), element('small', '', entry.detail));
      item.append(pip, copy, element('em', '', STATUS[entry.status]));
      host.appendChild(item);
    });
  }

  function setMessage(root, text, kind) {
    var node = byId(root, 'plan-message');
    if (!node) return;
    node.textContent = text || '';
    node.className = 'plan-message' + (kind ? ' is-' + kind : '');
  }

  function update(root) {
    var data = collect(root);
    var entries = selectedEntries(data);
    var validation = validatePlan(data);
    var count = byId(root, 'plan-selected-count');
    if (count) count.textContent = entries.length + ' 项已选择';
    GROUPS.forEach(function (group) {
      var node = root.querySelector('[data-plan-count="' + group.id + '"]');
      if (node) node.textContent = (data.selectedByGroup[group.id] || []).length + ' / ' + group.items.length;
    });
    var warningHost = byId(root, 'plan-dependency-list');
    if (warningHost) {
      warningHost.replaceChildren();
      validation.warnings.forEach(function (warning) {
        var li = document.createElement('li');
        li.textContent = warning.message + ' 缺少：' + warning.missing.join('、');
        warningHost.appendChild(li);
      });
      if (!validation.warnings.length) {
        var li = document.createElement('li'); li.textContent = '依赖检查通过；生成计划后仍需由执行者复核。'; warningHost.appendChild(li);
      }
    }
    return { data: data, validation: validation };
  }

  function invalidatePreview(root) {
    var preview = byId(root, 'plan-preview');
    if (!preview || !preview.value) return;
    var copy = byId(root, 'plan-copy');
    var download = byId(root, 'plan-download');
    if (copy) copy.disabled = true;
    if (download) download.disabled = true;
    setMessage(root, '选择或字段已修改。请重新生成，以免回传旧版本计划。', 'warn');
  }

  function copyText(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) return navigator.clipboard.writeText(text);
    return new Promise(function (resolve, reject) {
      var area = document.createElement('textarea'); area.value = text; area.style.position = 'fixed'; area.style.opacity = '0';
      document.body.appendChild(area); area.select();
      try { document.execCommand('copy'); resolve(); } catch (error) { reject(error); } finally { area.remove(); }
    });
  }

  function mount(root, options) {
    if (!root && typeof document !== 'undefined') root = document.querySelector('[data-rpi-plan-generator]');
    if (!root) return null;
    options = options || {};
    if (options.renderGroups !== false) renderGroups(root);
    renderCurrentStatus(root);
    var dateInput = byId(root, 'plan-date');
    if (dateInput && !dateInput.value) dateInput.value = today();
    var form = byId(root, 'plan-form');
    if (form) form.addEventListener('submit', function (event) { event.preventDefault(); });
    setAll(root, 'all');
    update(root);
    Array.prototype.slice.call(root.querySelectorAll('input, textarea, select')).forEach(function (input) { if (input.id === 'plan-preview') return; input.addEventListener('input', function () { update(root); invalidatePreview(root); }); input.addEventListener('change', function () { update(root); invalidatePreview(root); }); });
    var allButton = byId(root, 'plan-select-all'); if (allButton) allButton.addEventListener('click', function () { setAll(root, 'all'); update(root); setMessage(root, '已勾选全部确认范围。', 'ok'); invalidatePreview(root); });
    var recommended = byId(root, 'plan-select-recommended'); if (recommended) recommended.addEventListener('click', function () { setAll(root, 'recommended'); update(root); setMessage(root, '已切换为推荐首期：保留现有基线，优先补齐流式、上下文、重试与回归验收。', 'ok'); invalidatePreview(root); });
    var clear = byId(root, 'plan-clear'); if (clear) clear.addEventListener('click', function () { setAll(root, 'none'); update(root); setMessage(root, '已清空选择，请至少勾选一个范围后生成。', 'warn'); invalidatePreview(root); });
    var generate = byId(root, 'plan-generate'); if (generate) generate.addEventListener('click', function () {
      var state = update(root); 
      if (!state.validation.valid) { invalidatePreview(root); setMessage(root, state.validation.errors.join(' '), 'error'); return; }
      var text = generatePlanText(state.data); var preview = byId(root, 'plan-preview');
      if (preview) preview.value = text;
      var download = byId(root, 'plan-download'); if (download) download.disabled = false;
      var copy = byId(root, 'plan-copy'); if (copy) copy.disabled = false;
      setMessage(root, '确认计划已生成。请复制或下载后完整回传对话框，确认后再执行。', 'ok');
      if (typeof options.onGenerate === 'function') options.onGenerate(text, state.data);
    });
    var copy = byId(root, 'plan-copy'); if (copy) copy.addEventListener('click', function () { var preview = byId(root, 'plan-preview'); if (!preview || !preview.value) { setMessage(root, '请先生成计划文本。', 'warn'); return; } copyText(preview.value).then(function () { setMessage(root, '计划文本已复制到剪贴板。', 'ok'); }).catch(function () { setMessage(root, '浏览器拒绝剪贴板访问，请手动选择文本复制。', 'warn'); }); });
    var download = byId(root, 'plan-download'); if (download) download.addEventListener('click', function () { var preview = byId(root, 'plan-preview'); if (!preview || !preview.value) { setMessage(root, '请先生成计划文本。', 'warn'); return; } var blob = new Blob(['\uFEFF', preview.value], { type: 'text/plain;charset=utf-8' }); var url = URL.createObjectURL(blob); var anchor = document.createElement('a'); anchor.href = url; anchor.download = filename(collect(root)); document.body.appendChild(anchor); anchor.click(); anchor.remove(); setTimeout(function () { URL.revokeObjectURL(url); }, 0); setMessage(root, '已下载 UTF-8（含 BOM）计划文件：' + anchor.download, 'ok'); });
    var reset = byId(root, 'plan-reset'); if (reset) reset.addEventListener('click', function () { root.querySelector('form') && root.querySelector('form').reset(); setAll(root, 'all'); var preview = byId(root, 'plan-preview'); if (preview) preview.value = ''; if (byId(root, 'plan-copy')) byId(root, 'plan-copy').disabled = true; if (byId(root, 'plan-download')) byId(root, 'plan-download').disabled = true; update(root); setMessage(root, '已恢复默认完整路线。', 'ok'); });
    return { update: function () { return update(root); }, collect: function () { return collect(root); }, generate: function () { return generatePlanText(collect(root)); } };
  }

  global.RpiPlan = {
    mount: mount,
    collect: collect,
    generatePlanText: generatePlanText,
    validatePlan: validatePlan,
    computeDependencies: computeDependencies,
    filename: filename,
    groups: GROUPS,
    currentStatus: CURRENT_STATUS,
    statusLabels: STATUS
  };

  if (typeof document !== 'undefined') {
    document.addEventListener('DOMContentLoaded', function () {
      var root = document.querySelector('[data-rpi-plan-generator]');
      if (root) mount(root);
    });
  }
}(typeof window !== 'undefined' ? window : globalThis));
