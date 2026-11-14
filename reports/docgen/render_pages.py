# -*- coding: utf-8 -*-
"""三份 docx → PDF（LibreOffice 解包版）→ 页数统计 + 全页 PNG（供 visual-judge 验收）。

用法: python reports/docgen/render_pages.py [all|intro|tasks|manual]
输出: reports/out/pdf/<name>.pdf, reports/out/pages/<name>/pNNN.png
"""
import os, subprocess, sys, glob

SOFFICE = r"D:\ddcode\rpi\reports\tools\lo2\SourceDir\LibreOffice\program\soffice.com"
PROFILE = "file:///D:/ddcode/rpi/reports/tools/loprofile"
OUT = r"D:\ddcode\rpi\reports\out"
DOCS = {
    "intro": ("Samho-项目完整介绍.docx", 90),
    "tasks": ("Samho-多任务运行报告与分析.docx", 90),
    "manual": ("Samho-系统软件完整说明报告.docx", 110),
}

def to_pdf(docx_path, pdf_dir):
    os.makedirs(pdf_dir, exist_ok=True)
    subprocess.run([SOFFICE, "--headless", "--norestore", "-env:UserInstallation=" + PROFILE,
                    "--convert-to", "pdf", "--outdir", pdf_dir, docx_path],
                   check=True, timeout=600)

def main():
    sel = sys.argv[1] if len(sys.argv) > 1 else "all"
    keys = list(DOCS) if sel == "all" else [sel]
    pdf_dir = os.path.join(OUT, "pdf")
    os.makedirs(pdf_dir, exist_ok=True)
    for k in keys:
        fname, dpi = DOCS[k]
        docx_path = os.path.join(OUT, fname)
        print("converting", fname, "...")
        to_pdf(docx_path, pdf_dir)
        pdf_path = os.path.join(pdf_dir, fname.replace(".docx", ".pdf"))
        try:
            from pypdf import PdfReader
        except ImportError:
            subprocess.run([sys.executable, "-m", "pip", "install", "-q", "pypdf"], check=True)
            from pypdf import PdfReader
        n = len(PdfReader(pdf_path).pages)
        print(f"  {fname}: {n} pages")
        pages_dir = os.path.join(OUT, "pages", k)
        os.makedirs(pages_dir, exist_ok=True)
        for old in glob.glob(os.path.join(pages_dir, "*.png")):
            os.remove(old)
        subprocess.run(["pdftoppm", "-png", "-r", str(dpi), pdf_path,
                        os.path.join(pages_dir, "p")], check=True)
        pngs = sorted(glob.glob(os.path.join(pages_dir, "p*.png")))
        print(f"  rendered {len(pngs)} PNGs -> {pages_dir}")

if __name__ == "__main__":
    main()
