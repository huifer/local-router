#!/usr/bin/env node
/**
 * A high-concurrency mock upstream that speaks the OpenAI chat protocol.
 *
 * Unlike scripts/mock_vendor.py (HTTP/1.0, one thread per connection), this
 * one keeps connections alive and handles thousands of concurrent requests,
 * so a load test measures the gateway instead of the stub.
 *
 * Usage: node scripts/mock_vendor.mjs [--port 18101] [--delay-ms 0]
 *                                       [--model mock-small] [--fail 0]
 */
import http from "node:http";

const arg = (name, fallback) => {
  const index = process.argv.indexOf(`--${name}`);
  return index >= 0 && index + 1 < process.argv.length
    ? process.argv[index + 1]
    : fallback;
};

const port = Number.parseInt(arg("port", "18101"), 10);
const delayMs = Number.parseInt(arg("delay-ms", "0"), 10);
const model = arg("model", "mock-small");
const failRate = Number.parseInt(arg("fail", "0"), 10);
const reply = arg("reply", "pong from mock-vendor");

const server = http.createServer((request, response) => {
  let body = "";
  request.on("data", (chunk) => { body += chunk; });
  request.on("end", async () => {
    let payload = {};
    try { payload = JSON.parse(body || "{}"); } catch { /* ignore */ }

    if (request.method === "GET" && (request.url ?? "").endsWith("/models")) {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({
        object: "list",
        data: [{ id: model, object: "model" }],
      }));
      return;
    }

    if (failRate > 0 && Math.random() * 100 < failRate) {
      response.writeHead(500, { "content-type": "application/json" });
      response.end(JSON.stringify({ error: { message: "mock failure" } }));
      return;
    }

    const send = async () => {
      if (delayMs > 0) await new Promise((r) => setTimeout(r, delayMs));
      if (payload.stream) {
        response.writeHead(200, {
          "content-type": "text/event-stream",
          "cache-control": "no-cache",
        });
        const chunk = (delta, finish) =>
          `data: ${JSON.stringify({
            id: "chatcmpl-mock",
            object: "chat.completion.chunk",
            created: Math.floor(Date.now() / 1000),
            model: payload.model ?? model,
            choices: [{ index: 0, delta, finish_reason: finish }],
          })}\n\n`;
        response.write(chunk({ role: "assistant", content: "" }, null));
        response.write(chunk({ content: reply }, null));
        response.write(chunk({}, "stop"));
        response.end("data: [DONE]\n\n");
        return;
      }
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({
        id: "chatcmpl-mock",
        object: "chat.completion",
        created: Math.floor(Date.now() / 1000),
        model: payload.model ?? model,
        choices: [{
          index: 0,
          finish_reason: "stop",
          message: { role: "assistant", content: reply },
        }],
        usage: { prompt_tokens: 11, completion_tokens: 7, total_tokens: 18 },
      }));
    };

    send();
  });
});

// A large accept backlog: the default (511 on macOS) refuses connections
// under bursts, which shows up as gateway-side 502s.
server.listen(port, "127.0.0.1", 8192, () => {
  console.log(`[mock-vendor] listening on http://127.0.0.1:${port} delay=${delayMs}ms fail=${failRate}%`);
});
