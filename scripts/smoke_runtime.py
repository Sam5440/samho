"""跨平台真实服务冒烟。仅调用明确标注的模拟模型，不访问供应商。"""
import argparse
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import tempfile
import time
import urllib.request
import urllib.error


def request(base, path, body=None, method=None):
    data = None if body is None else json.dumps(body, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=5) as response:
        raw = response.read()
        if "application/json" in response.headers.get("Content-Type", ""):
            return json.loads(raw)
        return raw.decode("utf-8")


def main():
    parser = argparse.ArgumentParser(description="RPi 跨平台模拟服务冒烟")
    parser.add_argument("--binary", default="target/release/rpi")
    parser.add_argument("--report", default="reports/ci-smoke.json")
    parser.add_argument("--force", action="store_true", help="允许覆盖已存在的报告文件")
    parser.add_argument("--port", type=int, default=373)
    args = parser.parse_args()
    if not 300 <= args.port <= 400:
        raise SystemExit("测试服务端口必须在 300-400")
    binary = Path(args.binary).resolve()
    if os.name == "nt" and not binary.suffix:
        binary = binary.with_suffix(".exe")
    if not binary.is_file():
        raise SystemExit("请先构建 rpi 发布二进制")
    checks = []
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="rpi-smoke-") as root:
        root = Path(root)
        with socket.socket() as reservation:
            candidates = list(range(args.port, 401)) + list(range(300, args.port))
            for candidate in candidates:
                try:
                    reservation.bind(("127.0.0.1", candidate))
                    break
                except OSError:
                    continue
            else:
                raise RuntimeError("300-400 范围内没有可用测试端口")
            port = reservation.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        env = os.environ.copy()
        env.update(RPI_WORKSPACE=str(root), RPI_DATA_DIR=str(root / "data"), RPI_HTTP_BIND=f"127.0.0.1:{port}")
        log_path = root / "server.log"
        with log_path.open("wb") as output:
            process = subprocess.Popen([str(binary), "serve"], env=env, stdout=output, stderr=subprocess.STDOUT)
            try:
                deadline = time.monotonic() + 30
                while True:
                    if process.poll() is not None:
                        raise RuntimeError("服务提前退出：" + log_path.read_text(encoding="utf-8", errors="replace"))
                    try:
                        request(base, "/api/health")
                        break
                    except (OSError, urllib.error.URLError):
                        if time.monotonic() >= deadline:
                            raise TimeoutError("服务启动超时")
                        time.sleep(0.1)
                checks.append("真实二进制启动与健康")
                state = request(base, "/api/state")
                assert state["service"] == "rpi"
                assert state["simulation"] is True
                assert "中文" in request(base, "/docs")
                assert "确认" in request(base, "/plan")
                checks.append("中文文档/计划入口与模拟状态")
                task = request(base, "/api/run", {"prompt": "hello smoke test", "profile": {"complexity": 2, "automatic": False}, "skills": []})
                deadline = time.monotonic() + 15
                while True:
                    task = request(base, "/api/tasks/" + task["task_id"])
                    if task["status"] not in ("running", "queued"):
                        break
                    if time.monotonic() >= deadline:
                        raise TimeoutError("模拟任务超时")
                    time.sleep(0.05)
                result = task["result"]
                # 只检查结构化标识，不锚定完整本地化前缀（Demo 文案随版本演进）。
                assert "DEMO simulation" in result["output"]
                assert result["session_id"]
                checks.append("任务提交、真实轮询与明确模拟标记")
                session = request(base, "/api/sessions/" + result["session_id"])
                assert len(session["messages"]) >= 2
                page = request(base, "/api/events/page?after=0&limit=2")
                assert isinstance(page["events"], list) and len(page["events"]) == 2
                assert page["has_more"]
                checks.append("会话与游标轨迹页")
                ui = request(base, "/api/ui")
                assert all(key in ui for key in ("theme", "submit_key", "new_session_key", "cancel_key"))
                checks.append("主题/快捷键配置 API")
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
    report = {"platform": platform.platform(), "python": platform.python_version(),
              "binary": str(binary), "checks": checks, "elapsed_seconds": round(time.monotonic() - started, 3),
              "provider": "explicit demo simulation", "status": "passed"}
    destination = Path(args.report)
    if destination.exists() and not args.force:
        raise SystemExit(f"报告已存在：{destination}（使用 --force 覆盖）")
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": "passed", "checks": len(checks)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
