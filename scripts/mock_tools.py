#!/usr/bin/env python3
"""A mock vendor that answers with a tool call when the request carries tools."""
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass

    def _json(self, obj, status=200):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header('content-type', 'application/json')
        self.send_header('content-length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        n = int(self.headers.get('content-length', 0))
        req = json.loads(self.rfile.read(n) or b'{}')
        tools = req.get('tools') or []
        if tools:
            call = tools[0]
            self._json({
                "id": "chatcmpl-tool", "object": "chat.completion", "created": 0,
                "model": req.get("model", "mock"),
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": None, "tool_calls": [{
                        "id": "call_1", "type": "function",
                        "function": {"name": call["function"]["name"], "arguments": "{\"path\":\"/tmp/x\"}"},
                    }]},
                    "finish_reason": "tool_calls",
                }],
                "usage": {"prompt_tokens": 30, "completion_tokens": 12, "total_tokens": 42},
            })
            return
        self._json({
            "id": "chatcmpl-ok", "object": "chat.completion", "created": 0,
            "model": req.get("model", "mock"),
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7},
        })

if __name__ == '__main__':
    ThreadingHTTPServer(('127.0.0.1', int(sys.argv[1])), H).serve_forever()
