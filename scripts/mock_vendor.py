#!/usr/bin/env python3
"""A mock vendor that speaks the OpenAI chat format, for testing the gateway."""
import json, time
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

    def do_GET(self):
        if self.path.endswith('/models'):
            self._json({"object": "list", "data": [
                {"id": "mock-small", "object": "model"},
                {"id": "mock-big", "object": "model"},
            ]})
        else:
            self._json({"error": {"message": "not found"}})

    def do_POST(self):
        n = int(self.headers.get('content-length', 0))
        req = json.loads(self.rfile.read(n) or b'{}')
        if req.get('stream'):
            self.send_response(200)
            self.send_header('content-type', 'text/event-stream')
            self.end_headers()
            for piece in ['Hel', 'lo ', 'from ', 'the ', 'mock']:
                chunk = {"id": "chatcmpl-mock", "object": "chat.completion.chunk",
                         "created": 0, "model": req.get("model", "mock-small"),
                         "choices": [{"index": 0, "delta": {"content": piece}, "finish_reason": None}]}
                self.wfile.write(b'data: ' + json.dumps(chunk).encode() + b'\n\n')
                self.wfile.flush()
                time.sleep(0.01)
            done = {"id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 0,
                    "model": req.get("model", "mock-small"),
                    "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}
            self.wfile.write(b'data: ' + json.dumps(done).encode() + b'\n\n')
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()
            return
        msg = req.get('messages', [{}])[-1].get('content', '')
        self._json({
            "id": "chatcmpl-mock", "object": "chat.completion", "created": 0,
            "model": req.get("model", "mock-small"),
            "choices": [{"index": 0, "message": {"role": "assistant", "content": f"echo: {msg}"},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
        })

if __name__ == '__main__':
    ThreadingHTTPServer(('127.0.0.1', int(__import__('sys').argv[1])), H).serve_forever()
