# -*- coding: utf-8 -*-
"""修复 add_toc_placeholders.py 的首条目重复：TOC 域首段内含一个带空 PAGEREF
域的超链接条目（渲染无页码），其后紧跟一条同文本的纯文本占位段（带估算页码），
导致目录首条重复。修复 = 给空 PAGEREF 域注入该页码 + 删除占位段。
注意：TOC 域的 begin/instrText/separate 与首条目同段落，绝不能整段删除。

用法: python fix_toc_first.py <docx>   （就地修改）
"""
import re
import shutil
import sys
import zipfile

P_RE = re.compile(r"<w:p[ >].*?</w:p>", re.S)
T_RE = re.compile(r"<w:t[^>]*>([^<]*)</w:t>")

def visible_texts(p_xml):
    return "".join(T_RE.findall(p_xml)).strip()

def main(path):
    with zipfile.ZipFile(path) as z:
        names = z.namelist()
        data = {n: z.read(n) for n in names}
    xml = data["word/document.xml"].decode("utf-8")

    paras = list(P_RE.finditer(xml))
    drop = set()
    edits = {}
    removed = 0
    for idx, m in enumerate(paras):
        p = m.group(0)
        if (
            "PAGEREF" in p
            and 'w:fldCharType="separate"/><w:fldChar w:fldCharType="end"' in p
            and "<w:hyperlink" in p
        ):
            nxt = paras[idx + 1] if idx + 1 < len(paras) else None
            if nxt:
                a, b = visible_texts(p), visible_texts(nxt.group(0))
                if b.startswith(a) and b[len(a):].isdigit():
                    digits = b[len(a):]
                    edits[idx] = p.replace(
                        'w:fldCharType="separate"/><w:fldChar w:fldCharType="end"',
                        'w:fldCharType="separate"/><w:t xml:space="preserve">' + digits + "</w:t><w:fldChar w:fldCharType=\"end\"",
                        1,
                    )
                    drop.add(idx + 1)
                    removed += 1

    out = []
    pos = 0
    for idx, m in enumerate(paras):
        s, e = m.span()
        out.append(xml[pos:s])
        if idx in drop:
            pos = e
        else:
            out.append(edits.get(idx, m.group(0)))
            pos = e
    out.append(xml[pos:])
    xml2 = "".join(out)

    # 压缩 TOC 占位条目行距（before 120 / after 60 → 40 / 20），避免长目录溢出产生近空页
    xml2, nsp = re.subn(
        r'(<w:pStyle w:val="TOC[1-3]"/>(?:(?!</w:pPr>).)*?)<w:spacing w:before="120" w:after="60"/>',
        r'\1<w:spacing w:before="40" w:after="20"/>',
        xml2,
        flags=re.S,
    )
    print(f"fix_toc_first: removed {removed} duplicated first TOC entry(ies), compacted {nsp} TOC entries")

    data["word/document.xml"] = xml2.encode("utf-8")
    tmp = path + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:
        for n in names:
            z.writestr(n, data[n])
    shutil.move(tmp, path)

if __name__ == "__main__":
    main(sys.argv[1])
