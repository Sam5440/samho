"""Local bidirectional MCP fixture; never runs commands or calls a real provider."""
import json
import os
import sys
import threading

# MCP JSON-RPC uses UTF-8 even when Windows console defaults to another codepage.
sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8")

lock = threading.RLock()
condition = threading.Condition(lock)
responses = {}
counter = 0
client_info = {}
subscriptions = set()
roots_changed = 0
tasks = {}
modern = os.environ.get("MCP_TASKS") == "1"

def send(message):
    with lock:
        print(json.dumps(message), flush=True)

def ask(method, params):
    global counter
    with condition:
        counter += 1
        key = "server-" + str(counter)
        send({"jsonrpc": "2.0", "id": key, "method": method, "params": params})
        if not condition.wait_for(lambda: key in responses, timeout=5):
            return {"error": {"code": -32603, "message": "client callback did not arrive"}}
        return responses.pop(key)

def tool_result(value):
    return {"content": [{"type": "text", "text": json.dumps(value)}], "structuredContent": value}

def task_info(key, status, **extra):
    return {"taskId": key, "status": status, "createdAt": "2026-10-07T00:00:00Z", "lastUpdatedAt": "2026-10-07T00:00:00Z", "ttlMs": 60000, "pollIntervalMs": 10, **extra}

def handle(request):
    global client_info, roots_changed
    method = request.get("method")
    params = request.get("params", {})
    key = request.get("id")
    if key is None:
        if method == "notifications/roots/list_changed":
            roots_changed += 1
        return
    if method == "initialize":
        client_info = params
        capabilities = {"tools": {"listChanged": True}, "resources": {"subscribe": True, "listChanged": True}, "prompts": {"listChanged": True}, "completions": {}}
        if modern:
            capabilities["extensions"] = {"io.modelcontextprotocol/tasks": {}}
        result = {"protocolVersion": "2026-07-28" if modern else "2025-11-25", "capabilities": capabilities, "serverInfo": {"name": "optional-fixture", "version": "1"}}
    elif method == "tools/list":
        names = ["inspect_caps", "ask_sampling", "ask_elicitation", "ask_roots", "emit_changes", "task", "task_input", "slow_task"]
        tools = [{"name": name, "inputSchema": {"type": "object"}} for name in names]
        for name in ["schema", "wrong_output"]:
            tools.append({"name": name, "inputSchema": {"type": "object", "properties": {"value": {"type": "integer"}}, "required": ["value"], "additionalProperties": False}, "outputSchema": {"type": "object", "properties": {"value": {"type": "integer"}}, "required": ["value"], "additionalProperties": False}})
        result = {"tools": tools}
    elif method == "tools/call":
        name = params["name"]
        if name == "ask_sampling":
            result = tool_result(ask("sampling/createMessage", {"messages": [{"role": "user", "content": {"type": "text", "text": "local sampling fixture"}}], "maxTokens": params.get("arguments", {}).get("maxTokens", 16)}))
        elif name == "ask_elicitation":
            result = tool_result(ask("elicitation/create", {"mode": "form", "message": "Local fixture input", "requestedSchema": {"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"]}}))
        elif name == "ask_roots":
            result = tool_result(ask("roots/list", {}))
        elif name == "inspect_caps":
            result = tool_result({"client": client_info, "roots_changed": roots_changed})
        elif name == "emit_changes":
            for method_name in ["notifications/tools/list_changed", "notifications/resources/list_changed", "notifications/prompts/list_changed"]:
                send({"jsonrpc": "2.0", "method": method_name})
            result = tool_result({"emitted": True})
        elif name in ["task", "task_input", "slow_task"]:
            task_id = "t-" + str(key)
            tasks[task_id] = {"kind": name, "updated": False, "cancelled": False}
            result = {"resultType": "task", **task_info(task_id, "working")}
        elif name in ["schema", "wrong_output"]:
            result = tool_result({"value": params["arguments"]["value"] if name == "schema" else "invalid-output"})
        else:
            result = tool_result({"ok": True})
    elif method == "resources/subscribe":
        subscriptions.add(params["uri"])
        result = {}
        send({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": params["uri"]}})
    elif method == "resources/unsubscribe":
        subscriptions.discard(params["uri"])
        result = {}
    elif method == "completion/complete":
        result = {"completion": {"values": ["alpha", "beta"], "total": 2, "hasMore": False}}
    elif method == "tasks/get":
        task = tasks[params["taskId"]]
        if task["cancelled"]:
            result = {"resultType": "complete", **task_info(params["taskId"], "cancelled")}
        elif task["kind"] == "slow_task":
            result = {"resultType": "complete", **task_info(params["taskId"], "working")}
        elif task["kind"] == "task_input" and not task["updated"]:
            input_request = {"method": "elicitation/create", "params": {"mode": "form", "message": "Task fixture", "requestedSchema": {"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"]}}}
            result = {"resultType": "complete", **task_info(params["taskId"], "input_required", inputRequests={"question": input_request})}
        else:
            result = {"resultType": "complete", **task_info(params["taskId"], "completed", result=tool_result({"completed": True, "updated": task["updated"]}))}
    elif method == "tasks/update":
        tasks[params["taskId"]]["updated"] = True
        result = {"resultType": "complete"}
    elif method == "tasks/cancel":
        tasks[params["taskId"]]["cancelled"] = True
        result = {"resultType": "complete"}
    elif method in ["resources/list", "prompts/list"]:
        result = {"resources": []} if method == "resources/list" else {"prompts": []}
    elif method == "ping":
        result = {}
    else:
        send({"jsonrpc": "2.0", "id": key, "error": {"code": -32601, "message": "explicitly unsupported fixture method"}})
        return
    send({"jsonrpc": "2.0", "id": key, "result": result})

for line in sys.stdin:
    message = json.loads(line)
    if "method" not in message:
        with condition:
            responses[message["id"]] = {k: v for k, v in message.items() if k in ["result", "error"]}
            condition.notify_all()
    elif "id" in message:
        threading.Thread(target=handle, args=(message,), daemon=True).start()
    else:
        handle(message)
