#!/usr/bin/env python3
"""Minimal OpenAI-compatible mock upstream for testing airouter.

Usage:
  python3 upstream_stub.py PORT LATENCY_MS MODEL [MODEL...]

Serves /models, /v1/models, and /chat/completions (JSON + SSE), sleeping
LATENCY_MS before each completion to simulate generation speed.
"""
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def parse_args():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(1)
    port = int(sys.argv[1])
    latency_ms = int(sys.argv[2])
    models = sys.argv[3:]
    return port, latency_ms, models


PORT, LATENCY_MS, MODELS = parse_args()


def completion(model, stream):
    if stream:
        chunks = [
            {"id": "cmpl-1", "object": "chat.completion.chunk", "created": 0, "model": model,
             "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": None}]},
            {"id": "cmpl-1", "object": "chat.completion.chunk", "created": 0, "model": model,
             "choices": [{"index": 0, "delta": {"content": f"ready:{model}"}, "finish_reason": None}]},
            {"id": "cmpl-1", "object": "chat.completion.chunk", "created": 0, "model": model,
             "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
        ]
        out = "".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n"
        return out.encode("utf-8"), "text/event-stream"
    body = {
        "id": "cmpl-1",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": model,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": f"ready:{model}"},
                     "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 6, "completion_tokens": 1, "total_tokens": 7},
    }
    return json.dumps(body).encode("utf-8"), "application/json"


class Handler(BaseHTTPRequestHandler):
    def _send(self, code, body, ctype):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path in ("/models", "/v1/models"):
            data = {"object": "list", "data": [{"id": m, "object": "model"} for m in MODELS]}
            self._send(200, json.dumps(data).encode(), "application/json")
        else:
            self._send(404, b'{"error":{"message":"not found"}}', "application/json")

    def do_POST(self):
        if not self.path.endswith("/chat/completions"):
            self._send(404, b'{"error":{"message":"not found"}}', "application/json")
            return
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b"{}"
        try:
            req = json.loads(raw or b"{}")
        except Exception:
            req = {}
        model = req.get("model", MODELS[0] if MODELS else "unknown")
        stream = bool(req.get("stream"))
        time.sleep(LATENCY_MS / 1000.0)
        body, ctype = completion(model, stream)
        self._send(200, body, ctype)


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"mock upstream on 127.0.0.1:{PORT} models={MODELS} latency={LATENCY_MS}ms")
    srv.serve_forever()