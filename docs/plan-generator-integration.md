# 中文确认计划生成器集成说明

`web/plan.html` 是独立的本地中文计划编排页，使用 `web/plan.css` 和
`web/plan.js`。它只在浏览器本地读写表单、生成文本、复制到剪贴板和下载文件，
**不会调用 `/api/run`，不会启动 Agent 任务，也不会自动执行确认内容**。

用户应在页面勾选范围并生成计划，然后把完整文本文件（或文本全文）回传到对话框。
收到回传后，执行者再按范围、优先级、边界和验收门槛逐阶段执行。

## 独立页面

服务器应提供以下同源静态别名：

| 地址 | 文件 |
| --- | --- |
| `/plan`、`/plan.html` | `web/plan.html` |
| `/plan.css` | `web/plan.css` |
| `/plan.js` | `web/plan.js` |

三个文件也可以复制到同一目录，用浏览器直接打开 `plan.html` 离线使用。页面内链接
使用相对路径 `index.html` 和 `docs.html`，因此不会依赖固定端口。

## 挂载接口

`plan.html` 已包含以下根元素，宿主页面如需嵌入，只需保留这个属性并加载脚本：

```html
<div data-rpi-plan-generator>
  <!-- 与 web/plan.html 相同的表单标记。脚本会生成 #plan-groups。 -->
</div>
<script src="plan.js" defer></script>
```

脚本在 `DOMContentLoaded` 时自动查找 `[data-rpi-plan-generator]`。需要显式挂载时：

```js
const instance = window.RpiPlan.mount(document.querySelector('[data-rpi-plan-generator]'));
const state = instance.collect();
const text = instance.generate();
```

以下函数不依赖 DOM，适合 Node `vm` 或浏览器契约测试：

```js
window.RpiPlan.generatePlanText(data, { date: '2026-10-07' });
window.RpiPlan.validatePlan(data);
window.RpiPlan.computeDependencies(data);
window.RpiPlan.filename(data, { date: '2026-10-07' });
```

`generatePlanText` 的第二个参数可以固定 `date`，使测试输出确定。未传日期时使用本机
日期。下载文件名固定为 `rpi-确认执行计划-YYYY-MM-DD.txt`，内容使用 UTF-8 BOM，兼容
Windows 记事本。

## 数据形状

```js
{
  projectName: 'RPi · 可扩展 Rust Agent 框架',
  target: '项目目标（必填）',
  priority: '功能完整性与 Pi 任务执行语义',
  boundary: '执行边界',
  mode: '本地 Web GUI 与 TUI 共用 Rust 服务',
  sequence: '实施排序',
  models: '模型 / 供应商偏好（不放 API Key）',
  budget: '预算和延迟边界',
  testLevel: '契约 + 集成 + GUI / TUI smoke',
  deliverables: '交付物',
  operations: '允许和禁止的执行操作',
  logContent: false,
  platforms: ['Windows', 'macOS', 'Linux'],
  scenarios: ['项目分析 / 信息研究', '代码修改 / 调试 / 测试'],
  notes: '补充说明',
  date: '2026-10-07',
  selected: { 'object-lifecycle': true, 'web-gui': true }
}
```

实际页面包含 `#plan-project-name`、`#plan-target`、`#plan-priority`、
`#plan-boundary`、`#plan-mode`、`#plan-sequence`、`#plan-models`、
`#plan-budget`、`#plan-test-level`、`#plan-deliverables`、`#plan-operations`、
`#plan-log-content`、`#plan-notes`、`#plan-date`、`#plan-preview` 等标记。
范围项使用 `input[data-plan-item][data-plan-group]`；平台使用
`[data-plan-platform]`，场景使用 `[data-plan-scenario]`。

## 校验与依赖

`validatePlan(data)` 返回 `{ valid, errors, warnings }`：

- `errors` 要求目标、优先级和至少一个范围；错误存在时页面不会启用复制 / 下载。
- `warnings` 只提示范围依赖，例如选择 Web GUI 却未选择共用 Runtime、选择 MCP
  细项却未选择 MCP 客户端阶段。提示不会擅自修改用户选择。

生成文本会始终列出当前基线：首版已完成、部分完成和未完成能力；并列出本轮未勾选的
范围。因此“Provider 流式、上下文压缩、会话树 / fork、插件热替换、MCP 可选能力”等
规划项不会被表述为已经完成。

## 宿主路由建议

宿主 Axum 服务可以把 `/plan` 和 `/plan.html` 映射到 `plan.html`，把静态资源分别映射
到 `plan.css` 与 `plan.js`。若使用 `include_str!`，要显式设置：

```text
Content-Type: text/html; charset=utf-8
Content-Type: text/css; charset=utf-8
Content-Type: text/javascript; charset=utf-8
```

计划页不需要 API 状态，因此也可以在服务未启动时直接以文件方式打开。页面不应注入
任务提交权限或偷偷保存凭据；模型 Key、MCP Token 等只应留在本地配置 / 环境变量中。

