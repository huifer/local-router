#!/usr/bin/env node
/**
 * One worker process of the load generator. Forked by benchmark.mjs; it
 * keeps `--concurrency / --processes` connections open and reports its
 * latency samples back over IPC.
 */
import http from "node:http";
import fs from "node:fs";

const url = new URL(process.env.BENCH_URL ?? "http://127.0.0.1:3435");
const key = process.env.BENCH_KEY ?? "";
const model = process.env.BENCH_MODEL ?? "mock/mock-small";
const connections = Number.parseInt(process.env.BENCH_CONNECTIONS ?? "10", 10);
const seconds = Number.parseFloat(process.env.BENCH_SECONDS ?? "10");
const stream = process.env.BENCH_STREAM === "1";
const label = process.env.BENCH_LABEL ?? "worker";
const outPath = process.env.BENCH_OUT ?? "";

const agent = new http.Agent({
  keepAlive: true,
  maxSockets: connections,
  maxFreeSockets: connections,
  keepAliveMsecs: 30_000,
});

const payload = Buffer.from(JSON.stringify({
  model,
  messages: [{ role: "user", content: "hi, one short sentence please" }],
  max_tokens: 32,
  ...(stream ? { stream: true } : {}),
}));

const latencies = [];
const statuses = {};
const errorTypes = {};
let ok = 0;
let failed = 0;
const startedAt = performance.now();
const deadline = startedAt + seconds * 1000;

function requestOnce() {
  return new Promise((resolve) => {
    const requestStarted = performance.now();
    const request = http.request({
      host: url.hostname,
      port: url.port || (url.protocol === "https:" ? 443 : 80),
      path: "/v1/chat/completions",
      method: "POST",
      agent,
      headers: {
        "content-type": "application/json",
        "content-length": payload.length,
        connection: "keep-alive",
        "x-trace-id": `benchmark-${label}`,
        ...(key ? { authorization: `Bearer ${key}` } : {}),
      },
    }, (response) => {
      const status = String(response.statusCode ?? 0);
      statuses[status] = (statuses[status] ?? 0) + 1;
      response.resume();
      response.on("end", () => {
        if (response.statusCode === 200) {
          latencies.push(performance.now() - requestStarted);
          ok += 1;
        } else {
          failed += 1;
        }
        resolve();
      });
    });
    request.on("error", (error) => {
      failed += 1;
      const name = error.code || error.name || "request_error";
      errorTypes[name] = (errorTypes[name] ?? 0) + 1;
      resolve();
    });
    request.end(payload);
  });
}

async function worker() {
  while (performance.now() < deadline) {
    await requestOnce();
  }
}

await Promise.all(Array.from({ length: connections }, worker));
agent.destroy();

// The report travels through a file: process.send() followed by exit can
// truncate the IPC message, which leaves the parent waiting forever.
const report = JSON.stringify({
  latencies,
  statuses,
  error_types: errorTypes,
  ok,
  errors: failed,
});
if (outPath) {
  fs.writeFileSync(outPath, report);
} else {
  console.log(report);
}
