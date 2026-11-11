"""A deterministic MCP stdio peer; stdout contains protocol messages only."""
import json
import os
import sys
import threading
import time

lock = threading.Lock()
cancelled = set()
mode = os.environ.get("MCP_TEST_MODE", "normal")
pid_path = os.environ.get("MCP_PID_FILE")
if pid_path:
    with open(pid_path, "w", encoding="utf-8") as handle:
        handle.write(str(os.getpid()))

def send(message):
    with lock:
        print(json.dumps(message), flush=True)

def handle_request(request):
    global mode
    method = request["method"]
    params = request.get("params", {})
    request_id = request.get("id")
    if request_id is None:
        if method == "notifications/cancelled":
            cancelled.add(str(params["requestId"]))
            path = os.environ.get("MCP_CANCEL_FILE")
            if path:
                with open(path, "a", encoding="utf-8") as handle:
                    handle.write(json.dumps(params) + "\n")
        return
    if method == "initialize":
        if mode == "hang":
            return
        result = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {"listChanged": True}, "resources": {}, "prompts": {}}, "serverInfo": {"name": "samho-test", "version": "1"}}
    elif method == "tools/list":
        if mode == "collision":
            result = {"tools": [{"name": name, "inputSchema": {"type": "object"}} for name in ["a.b", "a_b"]]}
        elif params.get("cursor"):
            result = {"tools": [{"name": name, "description": name, "inputSchema": {"type": "object", "properties": {}}} for name in ["fail", "slow"]]}
        else:
            result = {"tools": [{"name": "echo", "description": "Echo arguments", "inputSchema": {"type": "object", "properties": {"value": {}}}}], "nextCursor": "page-two"}
    elif method == "tools/call":
        name = params["name"]
        if name == "change_collision":
            mode = "collision"
            send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
        if name == "remote_error":
            send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32602, "message": "invalid fixture arguments", "data": {"fixture": True}}})
            return
        if name == "slow":
            for _ in range(300):
                time.sleep(0.01)
                if str(request_id) in cancelled:
                    return
        result = {"content": [{"type": "text", "text": json.dumps(params.get("arguments", {}))}], "structuredContent": params.get("arguments", {}), "isError": name == "fail"}
    elif method == "resources/list":
        result = {"resources": [{"uri": "test://hello", "name": "hello", "mimeType": "text/plain"}]}
    elif method == "resources/templates/list":
        result = {"resourceTemplates": [{"uriTemplate": "test://{name}", "name": "greeting"}]}
    elif method == "resources/read":
        result = {"contents": [{"uri": params["uri"], "mimeType": "text/plain", "text": "fixture resource"}]}
    elif method == "prompts/list":
        result = {"prompts": [{"name": "greet", "description": "A test prompt", "arguments": [{"name": "name", "required": True}]}]}
    elif method == "prompts/get":
        result = {"description": "Fixture greeting", "messages": [{"role": "user", "content": {"type": "text", "text": "Hello " + params.get("arguments", {}).get("name", "world")}}]}
    elif method == "ping":
        result = {}
    else:
        send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32601, "message": "method unavailable"}})
        return
    send({"jsonrpc": "2.0", "id": request_id, "result": result})

for line in sys.stdin:
    request = json.loads(line)
    if "id" in request:
        threading.Thread(target=handle_request, args=(request,), daemon=True).start()
    else:
        handle_request(request)
