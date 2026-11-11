"""将中文 Markdown 文档打包为可离线阅读的网页资料；仅使用 Python 标准库。"""
from pathlib import Path
import json

ROOT = Path(__file__).resolve().parent.parent
CATALOG = [
    ("guide", "中文使用说明", "开始使用", "docs/中文使用说明.md"),
    ("confirmation", "确认计划操作说明", "开始使用", "docs/中文确认计划说明.md"),
    ("readme", "项目总览与启动", "开始使用", "README.md"),
    ("pi", "Pi 源码全面分析", "上游分析", "docs/pi-analysis.md"),
    ("references", "DSH 与 ZCode 参考分析", "上游分析", "docs/reference-analysis.md"),
    ("architecture", "Rust 运行时架构", "实现设计", "docs/architecture.md"),
    ("customization", "自定义配置与扩展", "实现设计", "docs/customization.md"),
    ("mcp", "MCP 支持能力与边界", "实现设计", "docs/mcp-support.md"),
    ("status", "实现状态与后续阶段", "实现设计", "docs/implementation-status.md"),
    ("validation", "多场景测试与性能验证", "验证与交付", "docs/validation-plan.md"),
    ("ports", "测试端口策略", "验证与交付", "docs/test-port-policy.md"),
    ("facets", "Remote Facet 远程扩展", "实现设计", "docs/remote-facets.md"),
]

def main():
    entries = []
    for doc_id, title, category, path in CATALOG:
        body = (ROOT / path).read_text(encoding="utf-8-sig")
        entries.append(dict(id=doc_id, title=title, category=category, path=path, body=body))
    output = "// 由 scripts/build-chinese-docs.py 生成；修改原始 Markdown 后重新运行。\nwindow.SamhoDocuments = " + json.dumps(entries, ensure_ascii=False) + ";\n"
    (ROOT / "web/documents-data.js").write_text(output, encoding="utf-8")
    print(f"已打包 {len(entries)} 份中文文档：web/documents-data.js")

if __name__ == "__main__":
    main()
