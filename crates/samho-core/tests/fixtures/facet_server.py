"""Deterministic authenticated Facet protocol fixture; no external network."""
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PROTOCOL = "samho-facet/1"
TOKEN = os.environ["SAMHO_FIXTURE_FACET_TOKEN"]
pending = {}
lock = threading.Lock()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, value):
        data = json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        if self.headers.get("Authorization") != "Bearer " + TOKEN:
            return self.reply(401, {"error": "authentication required"})
        size = int(self.headers.get("Content-Length", "0"))
        if size > 1024 * 1024:
            return self.reply(413, {"error": "request too large"})
        data = json.loads(self.rfile.read(size))
        if data.get("protocol") != PROTOCOL:
            return self.reply(400, {"error": "unsupported protocol"})
        if self.path == "/handshake":
            return self.reply(200, {"protocol": PROTOCOL, "name": "worker", "version": "fixture-1", "capabilities": ["echo", "slow"], "metadata": {"pid": os.getpid()}})
        call_id = data.get("call_id")
        if self.path == "/cancel":
            with lock:
                event = pending.get(call_id)
            if event:
                event.set()
            return self.reply(200, {"protocol": PROTOCOL, "call_id": call_id, "cancelled": bool(event)})
        if self.path != "/invoke" or data.get("method") not in ("echo", "slow"):
            return self.reply(404, {"error": "unknown capability"})
        if data["method"] == "slow":
            event = threading.Event()
            with lock:
                pending[call_id] = event
            event.wait(5)
            with lock:
                pending.pop(call_id, None)
        return self.reply(200, {"protocol": PROTOCOL, "call_id": call_id, "result": {"echo": data.get("input"), "trace_id": data.get("trace_id"), "session_id": data.get("session_id"), "parent_span_id": data.get("parent_span_id"), "pid": os.getpid()}})


port = int(os.environ.get("SAMHO_FIXTURE_FACET_PORT", "1399"))
if not 1300 <= port <= 1400:
    raise RuntimeError("SAMHO_FIXTURE_FACET_PORT must be within 1300-1400")
server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
