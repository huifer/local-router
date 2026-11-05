#!/usr/bin/env node
// 一个零依赖的桩服务，同时说 OpenAI Chat 和 Anthropic Messages 两种协议，
// 仅用于本地验证 local-router 的跨实例级联（agent → 香港网关 → 美国节点 → 上游）。
// 它代表「美国节点能访问到的上游」；真实场景里这一跳是 api.anthropic.com。
//
// 启动：node server.js        （默认 9000 端口，可用 PORT 覆盖）

const http = require("http");

const PORT = Number(process.env.PORT) || 9000;
const REPLY =
  "pong from mock-upstream (the upstream the US node reaches)";

// --------------------------------------------------------------- anthropic --

function anthropicMessage(model) {
  return {
    id: "msg_mock_" + Date.now(),
    type: "message",
    role: "assistant",
    model: model,
    content: [{ type: "text", text: REPLY }],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 5, output_tokens: 8 },
  };
}

function anthropicStream(res, model) {
  const id = "msg_mock_" + Date.now();
  const send = (event, data) => {
    res.write(`event: ${event}\n`);
    res.write(`data: ${JSON.stringify(data)}\n\n`);
  };

  res.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
    "x-mock-hop": "mock-upstream",
  });

  send("message_start", {
    type: "message_start",
    message: {
      id: id,
      type: "message",
      role: "assistant",
      model: model,
      content: [],
      stop_reason: null,
      stop_sequence: null,
      usage: { input_tokens: 5, output_tokens: 0 },
    },
  });
  send("content_block_start", {
    type: "content_block_start",
    index: 0,
    content_block: { type: "text", text: "" },
  });
  send("content_block_delta", {
    type: "content_block_delta",
    index: 0,
    delta: { type: "text_delta", text: REPLY },
  });
  send("content_block_stop", { type: "content_block_stop", index: 0 });
  send("message_delta", {
    type: "message_delta",
    delta: { stop_reason: "end_turn", stop_sequence: null },
    usage: { output_tokens: 8 },
  });
  send("message_stop", { type: "message_stop" });
  res.end();
}

// ------------------------------------------------------------------ openai --

function openaiChatCompletion(model) {
  return {
    id: "chatcmpl-mock-" + Date.now(),
    object: "chat.completion",
    created: Math.floor(Date.now() / 1000),
    model: model,
    choices: [
      {
        index: 0,
        finish_reason: "stop",
        message: { role: "assistant", content: REPLY },
      },
    ],
    usage: { prompt_tokens: 5, completion_tokens: 8, total_tokens: 13 },
  };
}

function openaiStream(res, model) {
  const id = "chatcmpl-mock-" + Date.now();
  const chunk = (delta, finish) =>
    "data: " +
    JSON.stringify({
      id: id,
      object: "chat.completion.chunk",
      created: Math.floor(Date.now() / 1000),
      model: model,
      choices: [{ index: 0, finish_reason: finish, delta: delta }],
    }) +
    "\n\n";

  res.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
    "x-mock-hop": "mock-upstream",
  });
  res.write(chunk({ role: "assistant" }, null));
  res.write(chunk({ content: REPLY }, null));
  res.write(chunk({}, "stop"));
  res.end("data: [DONE]\n\n");
}

// ------------------------------------------------------------------ server --

function readBody(req) {
  return new Promise((resolve) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        resolve(JSON.parse(body || "{}"));
      } catch (_) {
        resolve({});
      }
    });
  });
}

const server = http.createServer(async (req, res) => {
  const auth = req.headers["authorization"] || req.headers["x-api-key"] || "(none)";
  const url = (req.url || "").split("?")[0];

  if (req.method === "GET" && (url === "/v1/models" || url === "/v1beta/models")) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ object: "list", data: [] }));
    return;
  }

  if (req.method === "POST" && url === "/v1/messages") {
    const payload = await readBody(req);
    console.log(
      `[mock] anthropic model=${payload.model} auth=${String(auth).slice(0, 18)}…`
    );
    if (payload.stream) {
      anthropicStream(res, payload.model || "claude");
    } else {
      res.writeHead(200, {
        "content-type": "application/json",
        "x-mock-hop": "mock-upstream",
      });
      res.end(JSON.stringify(anthropicMessage(payload.model || "claude")));
    }
    return;
  }

  if (req.method === "POST" && url === "/v1/chat/completions") {
    const payload = await readBody(req);
    console.log(
      `[mock] openai model=${payload.model} auth=${String(auth).slice(0, 18)}…`
    );
    if (payload.stream) {
      openaiStream(res, payload.model || "claude");
    } else {
      res.writeHead(200, {
        "content-type": "application/json",
        "x-mock-hop": "mock-upstream",
      });
      res.end(JSON.stringify(openaiChatCompletion(payload.model || "claude")));
    }
    return;
  }

  res.writeHead(404, { "content-type": "application/json" });
  res.end(JSON.stringify({ error: "not found" }));
});

server.listen(PORT, () => {
  console.log(`[mock] stub listening on :${PORT} (anthropic + openai)`);
});
