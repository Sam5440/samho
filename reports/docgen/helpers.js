// Samho 报告共享生成库：DM-1 配色 + R1 封面配方 + 通用构件
const {
  Document, Packer, Paragraph, TextRun, Table, TableRow, TableCell,
  ImageRun, PageBreak, Header, Footer, PageNumber, NumberFormat,
  AlignmentType, HeadingLevel, WidthType, BorderStyle, ShadingType,
  TableLayoutType, SectionType, LevelFormat, TableOfContents, VerticalAlign,
} = require("docx");
const fs = require("fs");
const path = require("path");
const sizeOf = require("image-size");

// ── DM-1 Deep Cyan（科技/AI）──
const P = {
  bg: "162235", primary: "0F2233", accent: "1B6B7A",
  body: "000000", secondary: "5A6B7A",
  cover: { titleColor: "FFFFFF", subtitleColor: "B0B8C0", metaColor: "90989F", footerColor: "687078" },
  table: { headerBg: "1B6B7A", headerText: "FFFFFF", accentLine: "1B6B7A", innerLine: "C8DDE2", surface: "EDF3F5" },
};

const HEAD_FONT = { ascii: "Arial", eastAsia: "SimHei" };
const BODY_FONT = { ascii: "Calibri", eastAsia: "Microsoft YaHei" };
const MONO_FONT = { ascii: "Consolas", eastAsia: "Microsoft YaHei" };

const NB = { style: BorderStyle.NONE, size: 0, color: "FFFFFF" };
const noBorders = { top: NB, bottom: NB, left: NB, right: NB };
const allNoBorders = { top: NB, bottom: NB, left: NB, right: NB, insideHorizontal: NB, insideVertical: NB };

// ── 封面标题布局（design-system 标准实现）──
function splitTitleLines(title, charsPerLine) {
  if (title.length <= charsPerLine) return [title];
  const breakAfter = new Set([..."，。、；：！？", ..."的与和及之在于为", ..."-_—–·/", ..." \t"]);
  const lines = [];
  let remaining = title;
  while (remaining.length > charsPerLine) {
    let breakAt = -1;
    for (let i = charsPerLine; i >= Math.floor(charsPerLine * 0.6); i--) {
      if (i < remaining.length && breakAfter.has(remaining[i - 1])) { breakAt = i; break; }
    }
    if (breakAt === -1) {
      const limit = Math.min(remaining.length, Math.ceil(charsPerLine * 1.3));
      for (let i = charsPerLine + 1; i < limit; i++) {
        if (breakAfter.has(remaining[i - 1])) { breakAt = i; break; }
      }
    }
    if (breakAt === -1) {
      breakAt = charsPerLine;
      const prevChar = remaining[breakAt - 1], nextChar = remaining[breakAt];
      if (prevChar && nextChar && !breakAfter.has(prevChar) && !breakAfter.has(nextChar)
        && /[\u4e00-\u9fff]/.test(prevChar) && /[\u4e00-\u9fff]/.test(nextChar)) breakAt -= 1;
    }
    lines.push(remaining.slice(0, breakAt).trim());
    remaining = remaining.slice(breakAt).trim();
  }
  if (remaining) lines.push(remaining);
  if (lines.length > 1 && lines[lines.length - 1].length <= 2) {
    const last = lines.pop();
    lines[lines.length - 1] += last;
  }
  return lines;
}

function calcTitleLayout(title, maxWidthTwips, preferredPt = 40, minPt = 24) {
  const charWidth = (pt) => pt * 20;
  const charsPerLine = (pt) => Math.floor(maxWidthTwips / charWidth(pt));
  let titlePt = preferredPt, lines;
  while (titlePt >= minPt) {
    const cpl = charsPerLine(titlePt);
    if (cpl < 2) { titlePt -= 2; continue; }
    lines = splitTitleLines(title, cpl);
    if (lines.length <= 3) break;
    titlePt -= 2;
  }
  if (!lines || lines.length > 3) {
    lines = splitTitleLines(title, charsPerLine(minPt));
    titlePt = minPt;
  }
  return { titlePt, titleLines: lines };
}

function calcCoverSpacing(params) {
  const {
    titleLineCount = 1, titlePt = 36, hasSubtitle = false, hasEnglishLabel = false,
    metaLineCount = 0, fixedHeight = 800, pageHeight = 16838, marginTop = 0, marginBottom = 0,
  } = params;
  const SAFETY = 1200;
  const usableHeight = pageHeight - marginTop - marginBottom - SAFETY;
  const titleHeight = titleLineCount * (titlePt * 23 + 200);
  const subtitleHeight = hasSubtitle ? (12 * 23 + 600) : 0;
  const englishLabelHeight = hasEnglishLabel ? (9 * 23 + 600) : 0;
  const metaHeight = metaLineCount * (10 * 23 + 100);
  const implicitParaHeight = 3 * 300;
  const contentHeight = titleHeight + subtitleHeight + englishLabelHeight + metaHeight + fixedHeight + implicitParaHeight;
  const remainingSpace = usableHeight - contentHeight;
  const safeRemaining = Math.max(remainingSpace, 400);
  const FOOTER_MIN = 800;
  const rawTop = Math.floor(safeRemaining * 0.45);
  const rawBottom = Math.floor(safeRemaining * 0.45);
  const bottomSpacing = Math.max(rawBottom, FOOTER_MIN);
  const topSpacing = Math.max(rawTop - Math.max(0, FOOTER_MIN - rawBottom), 400);
  const midSpacing = Math.max(safeRemaining - topSpacing - bottomSpacing, 0);
  return { topSpacing, midSpacing, bottomSpacing };
}

// ── R1 封面（Pure Paragraph Left）──
function buildCoverR1(config) {
  const C = P.cover;
  const padL = 1200, padR = 800;
  const availableWidth = 11906 - padL - padR - 300;
  let { titlePt, titleLines } = calcTitleLayout(config.title, availableWidth, 40, 24);
  if (config.titleLinesOverride) {
    titleLines = config.titleLinesOverride;
    const longest = Math.max(...titleLines.map(l => l.length));
    titlePt = Math.min(40, Math.max(24, Math.floor(availableWidth / (longest * 20))));
  }
  const titleSize = titlePt * 2;
  const spacing = calcCoverSpacing({
    titleLineCount: titleLines.length, titlePt,
    hasSubtitle: !!config.subtitle, hasEnglishLabel: !!config.englishLabel,
    metaLineCount: (config.metaLines || []).length, fixedHeight: 400,
  });
  const accentLeft = { style: BorderStyle.SINGLE, size: 8, color: "37DCF2", space: 12 };
  const children = [];
  children.push(new Paragraph({ spacing: { before: spacing.topSpacing } }));
  if (config.englishLabel) {
    children.push(new Paragraph({
      indent: { left: padL, right: padR }, spacing: { after: 500 },
      border: { bottom: { style: BorderStyle.SINGLE, size: 6, color: "37DCF2", space: 8 } },
      children: [new TextRun({ text: config.englishLabel.split("").join("  "), size: 18, color: "37DCF2", font: { ascii: "Calibri", eastAsia: "SimHei" }, characterSpacing: 40 })],
    }));
  }
  for (let i = 0; i < titleLines.length; i++) {
    children.push(new Paragraph({
      indent: { left: padL },
      spacing: { after: i < titleLines.length - 1 ? 100 : 300, line: Math.ceil(titlePt * 23), lineRule: "atLeast" },
      children: [new TextRun({ text: titleLines[i], size: titleSize, bold: true, color: C.titleColor, font: { eastAsia: "SimHei", ascii: "Arial" } })],
    }));
  }
  if (config.subtitle) {
    children.push(new Paragraph({
      indent: { left: padL }, spacing: { after: 800 },
      children: [new TextRun({ text: config.subtitle, size: 24, color: C.subtitleColor, font: { eastAsia: "Microsoft YaHei", ascii: "Arial" } })],
    }));
  }
  for (const line of (config.metaLines || [])) {
    children.push(new Paragraph({
      indent: { left: padL + 200 }, spacing: { after: 80 },
      border: { left: accentLeft },
      children: [new TextRun({ text: line, size: 24, color: C.metaColor, font: { eastAsia: "Microsoft YaHei", ascii: "Arial" } })],
    }));
  }
  children.push(new Paragraph({ spacing: { before: spacing.bottomSpacing } }));
  children.push(new Paragraph({
    indent: { left: padL, right: padR },
    border: { top: { style: BorderStyle.SINGLE, size: 2, color: "37DCF2", space: 8 } },
    spacing: { before: 200 },
    children: [
      new TextRun({ text: config.footerLeft || "", size: 16, color: C.footerColor, font: { ascii: "Arial", eastAsia: "Microsoft YaHei" } }),
      new TextRun({ text: "                                        " }),
      new TextRun({ text: config.footerRight || "", size: 16, color: C.footerColor, font: { ascii: "Arial", eastAsia: "Microsoft YaHei" } }),
    ],
  }));
  return [new Table({
    width: { size: 100, type: WidthType.PERCENTAGE },
    layout: TableLayoutType.FIXED,
    borders: allNoBorders,
    rows: [new TableRow({
      height: { value: 16838, rule: "exact" },
      children: [new TableCell({ shading: { type: ShadingType.CLEAR, fill: P.bg }, borders: noBorders, children })],
    })],
  })];
}

// ── 正文构件 ──
let figNo = 0, tabNo = 0;

function h1(text) {
  return new Paragraph({
    heading: HeadingLevel.HEADING_1,
    keepNext: true,
    spacing: { before: 360, after: 160, line: 380, lineRule: "atLeast" },
    children: [new TextRun({ text, bold: true, size: 32, color: P.primary, font: HEAD_FONT })],
  });
}
function h2(text) {
  return new Paragraph({
    heading: HeadingLevel.HEADING_2,
    keepNext: true,
    spacing: { before: 280, after: 120, line: 340, lineRule: "atLeast" },
    children: [new TextRun({ text, bold: true, size: 28, color: P.primary, font: HEAD_FONT })],
  });
}
function h3(text) {
  return new Paragraph({
    heading: HeadingLevel.HEADING_3,
    keepNext: true,
    spacing: { before: 220, after: 100, line: 312 },
    children: [new TextRun({ text, bold: true, size: 24, color: P.primary, font: HEAD_FONT })],
  });
}
function p(text, opts = {}) {
  const runs = Array.isArray(text) ? text : [new TextRun({ text, size: 24, color: "000000", font: BODY_FONT })];
  return new Paragraph({
    alignment: AlignmentType.JUSTIFIED,
    indent: opts.noIndent ? undefined : { firstLine: 420 },
    spacing: { line: 312, after: opts.after ?? 80 },
    keepNext: opts.keepNext,
    children: runs,
  });
}
function runsOf(parts) {
  // parts: [{t, bold, color, mono}]
  return parts.map(x => new TextRun({
    text: x.t, bold: !!x.bold, size: x.mono ? 22 : 24,
    color: x.color || "000000",
    font: x.mono ? MONO_FONT : BODY_FONT,
  }));
}
function bullets(items) {
  return items.map(text => new Paragraph({
    bullet: { level: 0 },
    alignment: AlignmentType.JUSTIFIED,
    spacing: { line: 312, after: 60 },
    children: Array.isArray(text) ? runsOf(text) : [new TextRun({ text, size: 24, color: "000000", font: BODY_FONT })],
  }));
}
function numbered(items, ref) {
  return items.map(text => new Paragraph({
    numbering: { reference: ref, level: 0 },
    alignment: AlignmentType.JUSTIFIED,
    spacing: { line: 312, after: 60 },
    children: Array.isArray(text) ? runsOf(text) : [new TextRun({ text, size: 24, color: "000000", font: BODY_FONT })],
  }));
}

// 图片：保持纵横比，宽图 560px，竖图按高度 620px 反推
function figure(imgPath, caption, opts = {}) {
  const buf = fs.readFileSync(imgPath);
  const dim = sizeOf.imageSize ? sizeOf.imageSize(buf) : sizeOf(buf);
  let w, h;
  const maxW = opts.maxW || 560, maxH = opts.maxH || 640;
  if (dim.width / dim.height >= maxW / maxH) { w = maxW; h = Math.round(maxW * dim.height / dim.width); }
  else { h = maxH; w = Math.round(maxH * dim.width / dim.height); }
  figNo += 1;
  const out = [
    new Paragraph({
      alignment: AlignmentType.CENTER,
      spacing: { before: 120, after: 60 },
      keepNext: true,
      children: [new ImageRun({ data: buf, transformation: { width: w, height: h }, type: "png" })],
    }),
    new Paragraph({
      alignment: AlignmentType.CENTER,
      spacing: { after: 160, line: 280 },
      children: [new TextRun({ text: "图 " + figNo + "  " + caption, size: 21, color: P.secondary, font: BODY_FONT })],
    }),
  ];
  return out;
}

// 表格：Horizontal-Only 商务样式
function tbl({ headers, rows, widths, caption, small }) {
  const fontSize = small ? 18 : 21;
  const n = headers.length;
  const ws = widths || Array(n).fill(Math.floor(100 / n));
  // keepNext 链：表头行与首行数据必须同页（防 LibreOffice「表头孤悬页底、数据行丢失」渲染 bug）
  const mk = (text, isHead, i, keepNext) => new TableCell({
    width: { size: ws[i], type: WidthType.PERCENTAGE },
    shading: isHead ? { type: ShadingType.CLEAR, fill: P.table.headerBg } : undefined,
    margins: { top: 70, bottom: 70, left: 110, right: 110 },
    verticalAlign: VerticalAlign.CENTER,
    children: [new Paragraph({
      keepNext,
      spacing: { line: 280 },
      children: [new TextRun({
        text: String(text), bold: isHead, size: fontSize,
        color: isHead ? P.table.headerText : "000000",
        font: BODY_FONT,
      })],
    })],
  });
  tabNo += 1;
  const out = [];
  if (caption) out.push(new Paragraph({
    keepNext: true, spacing: { before: 140, after: 80, line: 280 },
    children: [new TextRun({ text: "表 " + tabNo + "  " + caption, bold: true, size: 21, color: P.primary, font: BODY_FONT })],
  }));
  out.push(new Table({
    width: { size: 100, type: WidthType.PERCENTAGE },
    borders: {
      top: { style: BorderStyle.SINGLE, size: 4, color: P.table.accentLine },
      bottom: { style: BorderStyle.SINGLE, size: 4, color: P.table.accentLine },
      left: NB, right: NB,
      insideHorizontal: { style: BorderStyle.SINGLE, size: 1, color: P.table.innerLine },
      insideVertical: NB,
    },
    rows: [
      new TableRow({ tableHeader: true, cantSplit: true, children: headers.map((t, i) => mk(t, true, i, true)) }),
      ...rows.map((r, ri) => new TableRow({ cantSplit: true, children: r.map((t, i) => mk(t, false, i, ri === 0)) })),
    ],
  }));
  out.push(new Paragraph({ spacing: { after: 120 }, children: [] }));
  return out;
}

// 代码/命令块：左侧强调线 + 浅底
function codeBlock(lines) {
  return lines.map((line, i) => new Paragraph({
    shading: { type: ShadingType.CLEAR, fill: "F2F6F8" },
    border: { left: { style: BorderStyle.SINGLE, size: 10, color: P.accent, space: 10 } },
    indent: { left: 240 },
    spacing: { line: 276, before: i === 0 ? 100 : 0, after: i === lines.length - 1 ? 140 : 0 },
    keepLines: true,
    children: [new TextRun({ text: line.length ? line : " ", size: 20, color: "1A3A44", font: MONO_FONT })],
  }));
}

// 要点卡片（浅底段落）
function callout(title, text) {
  return [
    new Paragraph({
      shading: { type: ShadingType.CLEAR, fill: P.table.surface },
      border: { left: { style: BorderStyle.SINGLE, size: 12, color: P.accent, space: 10 } },
      indent: { left: 240, right: 240 },
      spacing: { before: 140, after: 40, line: 312 },
      keepNext: true,
      children: [new TextRun({ text: title, bold: true, size: 24, color: P.primary, font: BODY_FONT })],
    }),
    new Paragraph({
      shading: { type: ShadingType.CLEAR, fill: P.table.surface },
      border: { left: { style: BorderStyle.SINGLE, size: 12, color: P.accent, space: 10 } },
      indent: { left: 240, right: 240 },
      spacing: { after: 160, line: 312 },
      alignment: AlignmentType.JUSTIFIED,
      children: [new TextRun({ text, size: 22, color: "1A2B36", font: BODY_FONT })],
    }),
  ];
}

function pageFooter() {
  return new Footer({
    children: [new Paragraph({
      alignment: AlignmentType.CENTER,
      children: [new TextRun({ children: [PageNumber.CURRENT], size: 18, color: P.secondary, font: BODY_FONT })],
    })],
  });
}
function docHeader(title) {
  return new Header({
    children: [new Paragraph({
      alignment: AlignmentType.RIGHT,
      border: { bottom: { style: BorderStyle.SINGLE, size: 2, color: P.table.innerLine, space: 4 } },
      children: [new TextRun({ text: title, size: 18, color: P.secondary, font: BODY_FONT })],
    })],
  });
}

function tocBlock() {
  return [
    new Paragraph({
      alignment: AlignmentType.CENTER,
      spacing: { before: 480, after: 360 },
      children: [new TextRun({ text: "目  录", bold: true, size: 32, font: HEAD_FONT, color: P.primary })],
    }),
    new TableOfContents("Table of Contents", { hyperlink: true, headingStyleRange: "1-3" }),
    new Paragraph({
      spacing: { before: 200 },
      children: [new TextRun({
        text: "注：本目录由域代码生成。编辑后如需刷新页码，请在目录上右键选择「更新域」。",
        italics: true, size: 18, color: "888888", font: BODY_FONT,
      })],
    }),
  ];
}

// 三节装配：封面 / 前置(罗马页码+目录) / 正文(阿拉伯页码从 1)
function buildDoc({ coverConfig, headerTitle, frontExtra, bodyChildren, numbering }) {
  const front = [...(frontExtra || []), ...tocBlock()];
  return new Document({
    styles: {
      default: {
        document: {
          run: { font: BODY_FONT, size: 24, color: "000000" },
          paragraph: { spacing: { line: 312 } },
        },
        heading1: { run: { font: HEAD_FONT, size: 32, bold: true, color: P.primary }, paragraph: { spacing: { before: 360, after: 160 }, outlineLevel: 0 } },
        heading2: { run: { font: HEAD_FONT, size: 28, bold: true, color: P.primary }, paragraph: { spacing: { before: 280, after: 120 }, outlineLevel: 1 } },
        heading3: { run: { font: HEAD_FONT, size: 24, bold: true, color: P.primary }, paragraph: { spacing: { before: 220, after: 100 }, outlineLevel: 2 } },
      },
    },
    numbering: { config: numbering || [] },
    sections: [
      {
        properties: { page: { size: { width: 11906, height: 16838 }, margin: { top: 0, bottom: 0, left: 0, right: 0 } } },
        children: buildCoverR1(coverConfig),
      },
      {
        properties: {
          type: SectionType.NEXT_PAGE,
          page: {
            size: { width: 11906, height: 16838 },
            margin: { top: 1440, bottom: 1440, left: 1701, right: 1417 },
            pageNumbers: { start: 1, formatType: NumberFormat.UPPER_ROMAN },
          },
        },
        footers: { default: pageFooter() },
        children: front,
      },
      {
        properties: {
          type: SectionType.NEXT_PAGE,
          page: {
            size: { width: 11906, height: 16838 },
            margin: { top: 1440, bottom: 1440, left: 1701, right: 1417 },
            pageNumbers: { start: 1, formatType: NumberFormat.DECIMAL },
          },
        },
        headers: { default: docHeader(headerTitle) },
        footers: { default: pageFooter() },
        children: bodyChildren,
      },
    ],
  });
}

async function writeDocx(doc, outPath) {
  const buf = await Packer.toBuffer(doc);
  fs.writeFileSync(outPath, buf);
  console.log("written:", outPath, buf.length, "bytes");
}

module.exports = {
  P, HEAD_FONT, BODY_FONT, MONO_FONT, NB, noBorders, allNoBorders,
  buildCoverR1, calcTitleLayout, splitTitleLines, calcCoverSpacing,
  h1, h2, h3, p, runsOf, bullets, numbered, figure, tbl, codeBlock, callout,
  pageFooter, docHeader, tocBlock, buildDoc, writeDocx,
  resetCounters: () => { figNo = 0; tabNo = 0; },
  Document, Packer, Paragraph, TextRun, Table, TableRow, TableCell, ImageRun,
  PageBreak, AlignmentType, HeadingLevel, WidthType, BorderStyle, ShadingType,
  TableLayoutType, SectionType, LevelFormat, NumberFormat,
};
