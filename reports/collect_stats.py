# -*- coding: utf-8 -*-
"""汇总演示实例的会话/任务/事件/budget 数据，输出 UTF-8 JSON 摘要（reports/stats.json）。"""
import json, urllib.request, collections, datetime

BASE = "http://127.0.0.1:7433"
DATA = "reports/demo-data"

def get(path):
    with urllib.request.urlopen(BASE + path) as r:
        return json.loads(r.read().decode("utf-8"))

def jsonl(path):
    out = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                out.append(json.loads(line))
    return out

sessions_api = get("/api/sessions")
tasks_api = get("/api/tasks")
events = jsonl(f"{DATA}/events.jsonl")
budget = jsonl(f"{DATA}/budget.jsonl")

# 事件 kind 分布
kind_counter = collections.Counter(e.get("kind") for e in events)

# 工具调用统计（tool.call / tool.result）
tool_calls = []   # {name, session, latency_ms, is_error, ts}
for e in events:
    if e.get("kind") == "tool.result":
        p = e.get("payload", {})
        tool_calls.append({
            "ts": e.get("timestamp_ms"),
            "name": p.get("name") or (e.get("object_id") or "").split(":")[-1],
            "session": e.get("session_id"),
            "latency_ms": p.get("latency_ms"),
            "is_error": bool(p.get("is_error")),
        })

tools_counter = collections.Counter(c["name"] for c in tool_calls)
tool_errors = collections.Counter(c["name"] for c in tool_calls if c["is_error"])
lat_by_tool = collections.defaultdict(list)
for c in tool_calls:
    if isinstance(c["latency_ms"], (int, float)):
        lat_by_tool[c["name"]].append(c["latency_ms"])
lat_stats = {k: {"n": len(v), "p50": sorted(v)[len(v)//2], "max": max(v), "sum_ms": sum(v)} for k, v in lat_by_tool.items()}

# 每会话时间线：任务开始/结束、工具调用数
sess_events = collections.defaultdict(lambda: {"task_starts": [], "task_ends": [], "tool_results": 0, "first_ts": None, "last_ts": None})
for e in events:
    sid = e.get("session_id")
    k = e.get("kind")
    s = sess_events[sid]
    ts = e.get("timestamp_ms")
    if s["first_ts"] is None or ts < s["first_ts"]:
        s["first_ts"] = ts
    if s["last_ts"] is None or ts > s["last_ts"]:
        s["last_ts"] = ts
    if k == "task.start":
        s["task_starts"].append(ts)
    elif k in ("task.end", "task.complete", "task.finish"):
        s["task_ends"].append(ts)
    elif k == "tool.result":
        s["tool_results"] += 1

# 会话摘要（API 视角 + 事件视角）
sess_summary = []
for s in sessions_api:
    ev = sess_events.get(s["id"], {})
    dur = (s.get("updated_at", 0) - s.get("created_at", 0)) / 1000.0
    sess_summary.append({
        "id": s["id"][:8],
        "title": (s.get("title") or "")[:42],
        "status": s.get("status"),
        "messages": s.get("message_count"),
        "model": s.get("last_model"),
        "duration_s": round(dur, 1),
        "created": datetime.datetime.fromtimestamp(s["created_at"] / 1000).strftime("%H:%M:%S"),
        "tool_results": ev.get("tool_results", 0),
        "events": sum(1 for e in events if e.get("session_id") == s["id"]),
    })
sess_summary.sort(key=lambda x: x["created"])

# 任务摘要
task_summary = []
for t in tasks_api.get("tasks", []):
    task_summary.append({
        "id": (t.get("task_id") or t.get("id") or "?")[:8],
        "session": (t.get("session_id") or "")[:8],
        "status": t.get("status"),
        "has_error": bool(t.get("error")),
    })

# budget 走势（时间戳 + 累计 tokens）
budget_curve = [{"ts": b.get("timestamp_ms") or i, "tokens": b.get("tokens"), "usd": b.get("usd")} for i, b in enumerate(budget)]

summary = {
    "generated_at": datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
    "session_count": len(sessions_api),
    "sessions": sess_summary,
    "tasks": task_summary,
    "task_status_counter": dict(collections.Counter(t["status"] for t in task_summary)),
    "event_count": len(events),
    "event_kinds": dict(kind_counter),
    "tool_calls_total": len(tool_calls),
    "tool_calls_by_name": dict(tools_counter),
    "tool_errors_by_name": dict(tool_errors),
    "tool_latency_stats": lat_stats,
    "budget_entries": len(budget),
    "budget_last": budget[-1] if budget else None,
    "budget_curve": budget_curve,
}

with open("reports/stats.json", "w", encoding="utf-8") as f:
    json.dump(summary, f, ensure_ascii=False, indent=2)

print("sessions:", len(sessions_api), "| events:", len(events), "| tool_results:", len(tool_calls))
print("kinds:", dict(kind_counter))
print("tools:", dict(tools_counter))
print("errors:", dict(tool_errors))
print("latency:", json.dumps(lat_stats, ensure_ascii=False))
print("task_status:", summary["task_status_counter"])
print("budget_last:", budget[-1] if budget else None)
