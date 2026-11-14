#!/bin/bash
# 生成 + TOC 占位 + 页脚修复 + 质检 一条龙
set -e
DOCX_SCRIPTS="C:/Users/11025/.zcode/cli/plugins/cache/zcode-plugins-official/documents/0.2.2/skills/docx/scripts"
GEN="$1"; OUT="$2"
cd /d/ddcode/rpi
node "reports/docgen/$GEN"
python "$DOCX_SCRIPTS/add_toc_placeholders.py" "$OUT" --auto | tail -2
python "reports/docgen/fix_toc_first.py" "$OUT"
python "$DOCX_SCRIPTS/fix_footer_fields.py" "$OUT" 2>&1 | tail -2
python "$DOCX_SCRIPTS/postcheck.py" "$OUT" 2>&1 | tail -16
