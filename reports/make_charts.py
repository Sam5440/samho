# -*- coding: utf-8 -*-
"""生成报告用图表（matplotlib，中文字体 Microsoft YaHei），输出到 reports/charts/。"""
import json, collections, os
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib import font_manager

font_manager.fontManager.addfont(r"C:\Windows\Fonts\msyh.ttc")
plt.rcParams["font.family"] = "Microsoft YaHei"
plt.rcParams["axes.unicode_minus"] = False

os.makedirs("reports/charts", exist_ok=True)
stats = json.load(open("reports/stats.json", encoding="utf-8"))

INK = "#1f2937"; ACCENT = "#0f766e"; GRAY = "#9ca3af"; WARN = "#b45309"; SOFT = "#e5e7eb"

def style(ax):
    ax.spines["top"].set_visible(False)
    ax.spines["right"].set_visible(False)
    ax.spines["left"].set_color(SOFT)
    ax.spines["bottom"].set_color(SOFT)
    ax.tick_params(colors=INK, labelsize=10)
    ax.grid(axis="x" if getattr(ax, "_hbar", False) else "y", color=SOFT, lw=0.6, alpha=0.7)
    ax.set_axisbelow(True)

# ---- 图1：工具调用分布 ----
tools = stats["tool_calls_by_name"]
errs = stats["tool_errors_by_name"]
names = sorted(tools, key=lambda k: -tools[k])
vals = [tools[k] for k in names]
errv = [errs.get(k, 0) for k in names]
fig, ax = plt.subplots(figsize=(8.6, 4.2), dpi=200)
ax._hbar = True
y = range(len(names))[::-1]
ax.barh(y, vals, color=ACCENT, height=0.62, label="调用次数")
ax.barh(y, errv, color=WARN, height=0.62, label="失败次数")
for yi, v, e in zip(y, vals, errv):
    ax.text(v + 0.4, yi, f"{v}" + (f"（失败 {e}）" if e else ""), va="center", fontsize=10, color=INK)
ax.set_yticks(list(y)); ax.set_yticklabels(names, fontsize=11)
ax.set_xlim(0, max(vals) * 1.25)
ax.set_xlabel("调用次数", fontsize=10)
ax.set_title("演示期工具调用分布（共 68 次）", fontsize=13, color=INK, pad=10, loc="left")
ax.legend(frameon=False, fontsize=10)
style(ax)
fig.tight_layout(); fig.savefig("reports/charts/tool-dist.png", facecolor="white"); plt.close(fig)

# ---- 图2：工具延迟（p50 / max） ----
lat = stats["tool_latency_stats"]
lnames = [k for k in names if k in lat]
p50 = [lat[k]["p50"] for k in lnames]
mx = [lat[k]["max"] for k in lnames]
import numpy as np
x = np.arange(len(lnames))
fig, ax = plt.subplots(figsize=(8.6, 4.2), dpi=200)
w = 0.36
ax.bar(x - w/2, p50, w, color=ACCENT, label="p50 延迟")
ax.bar(x + w/2, mx, w, color="#155e75", alpha=0.75, label="最大延迟")
ax.set_yscale("log")
for xi, v in zip(x - w/2, p50): ax.text(xi, v * 1.15, f"{v}ms", ha="center", fontsize=8.5, color=INK)
for xi, v in zip(x + w/2, mx): ax.text(xi, v * 1.15, f"{v}ms", ha="center", fontsize=8.5, color=INK)
ax.set_xticks(x); ax.set_xticklabels(lnames, fontsize=11)
ax.set_ylabel("毫秒（对数刻度）", fontsize=10)
ax.set_title("工具执行延迟：本地文件工具毫秒级，shell 受进程启动拖慢", fontsize=13, color=INK, pad=10, loc="left")
ax.legend(frameon=False, fontsize=10)
style(ax)
fig.tight_layout(); fig.savefig("reports/charts/tool-latency.png", facecolor="white"); plt.close(fig)

# ---- 图3：budget 累计 token 走势 ----
# budget.jsonl 不含时间戳，只有逐条累计记录：横轴用记录序号（诚实口径）
curve = stats["budget_curve"]
tk = [c["tokens"] / 1000.0 for c in curve]
tx = list(range(len(curve)))
fig, ax = plt.subplots(figsize=(8.6, 4.0), dpi=200)
ax.plot(tx, tk, color=ACCENT, lw=2.2)
ax.fill_between(tx, tk, color=ACCENT, alpha=0.12)
ax.set_xlabel("预算记录序号（随模型响应逐条累计，无时间戳字段）", fontsize=10)
ax.set_ylabel("累计消耗（千 token）", fontsize=10)
ax.set_title("预算累计走势：17 个任务共消耗 236,676 token（$0.0，本地网关）", fontsize=13, color=INK, pad=10, loc="left")
ax.annotate(f"{tk[-1]:.1f}k", (tx[-1], tk[-1]), textcoords="offset points", xytext=(8, 0), fontsize=10, color=ACCENT, va="center")
style(ax)
fig.tight_layout(); fig.savefig("reports/charts/budget-curve.png", facecolor="white"); plt.close(fig)

# ---- 图4：任务并发 Gantt（按会话） ----
events = []
with open("reports/demo-data/events.jsonl", encoding="utf-8") as f:
    for line in f:
        if line.strip():
            events.append(json.loads(line))
starts = {}; ends = {}
for e in events:
    k = e.get("kind"); sid = e.get("session_id"); ts = e.get("timestamp_ms")
    if k == "task.start": starts.setdefault(sid, ts)
    if k == "task.done": ends[sid] = ts
rows = sorted(starts.keys(), key=lambda s: starts[s])
t0 = min(starts.values())
fig, ax = plt.subplots(figsize=(8.6, 5.6), dpi=200)
labels = []
for i, sid in enumerate(rows):
    st = (starts[sid] - t0) / 1000.0
    en = (ends.get(sid, starts[sid] + 60000) - t0) / 1000.0
    ax.barh(i, en - st, left=st, height=0.55, color=ACCENT if (en - st) < 120 else "#155e75", alpha=0.85)
    labels.append(sid[:8])
ax.set_yticks(range(len(rows))); ax.set_yticklabels(labels, fontsize=9)
ax.set_xlabel("相对第一个任务启动的时间（秒）", fontsize=10)
ax.set_title("17 个任务的执行时间窗（Gantt）：并发批次清晰可见，单任务 12–80 秒", fontsize=13, color=INK, pad=10, loc="left")
style(ax)
ax.grid(axis="x", color=SOFT, lw=0.6, alpha=0.7)
fig.tight_layout(); fig.savefig("reports/charts/task-gantt.png", facecolor="white"); plt.close(fig)

# ---- 图5：事件类型分布 ----
kinds = stats["event_kinds"]
kn = sorted(kinds, key=lambda k: -kinds[k])
kv = [kinds[k] for k in kn]
fig, ax = plt.subplots(figsize=(8.6, 4.2), dpi=200)
bars = ax.bar(range(len(kn)), kv, color=[ACCENT, "#155e75", "#0e7490", "#0891b2", "#14b8a6", "#2dd4bf", "#5eead4", "#99f6e4", GRAY][:len(kn)], width=0.62)
for xi, v in zip(range(len(kn)), kv):
    ax.text(xi, v + 1.2, str(v), ha="center", fontsize=10, color=INK)
ax.set_xticks(range(len(kn))); ax.set_xticklabels(kn, fontsize=9.5, rotation=18, ha="right")
ax.set_ylabel("事件数", fontsize=10)
ax.set_title("调用轨迹事件分布（共 325 条）：每次任务 1 次路由 + 若干模型/工具往返", fontsize=13, color=INK, pad=10, loc="left")
style(ax)
fig.tight_layout(); fig.savefig("reports/charts/event-kinds.png", facecolor="white"); plt.close(fig)

# ---- 图6：会话消息数 vs 工具调用 ----
sess = stats["sessions"]
fig, ax = plt.subplots(figsize=(8.6, 4.4), dpi=200)
xs = [s["messages"] for s in sess]
ys = [s["tool_results"] for s in sess]
ax.scatter(xs, ys, s=110, color=ACCENT, alpha=0.85, edgecolors="white", linewidths=1.2)
for s in sess:
    if s["messages"] >= 20:
        ax.annotate(f"{s['id']}（{s['messages']} 条消息）", (s["messages"], s["tool_results"]), textcoords="offset points", xytext=(10, 6), fontsize=9, color=INK)
ax.set_xlabel("会话消息数", fontsize=10)
ax.set_ylabel("工具结果事件数", fontsize=10)
ax.set_title("会话规模分布：多数单任务会话 5–9 条消息，超长会话 23 条（多轮续接）", fontsize=13, color=INK, pad=10, loc="left")
style(ax)
fig.tight_layout(); fig.savefig("reports/charts/session-scatter.png", facecolor="white"); plt.close(fig)

print("charts done:", os.listdir("reports/charts"))
