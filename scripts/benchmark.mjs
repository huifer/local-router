#!/usr/bin/env node
/**
 * A multi-process load generator for local-router.
 *
 * One Node process saturates its own event loop long before a 15-core
 * machine saturates the gateway, and a single process also runs out of
 * ephemeral ports (EADDRNOTAVAIL) at a few thousand connections. So this
 * forks N worker processes, each with its own connection budget, and merges
 * their latency samples into one result.
 *
 * Example:
 *   node scripts/benchmark.mjs \
 *     --url http://127.0.0.1:18100 --key bench-caller \
 *     --model mock/mock-small --concurrency 1000 --seconds 30 \
 *     --processes 4 --label gw-c1000
 */
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import { fork } from "node:child_process";
import { fileURLToPath } from "node:url";

const arg = (name, fallback) => {
  const index = process.argv.indexOf(`--${name}`);
  return index >= 0 && index + 1 < process.argv.length
    ? process.argv[index + 1]
    : fallback;
};
const flag = (name) => process.argv.includes(`--${name}`);

const url = new URL(arg("url", "http://127.0.0.1:3435"));
const key = arg("key", "");
const model = arg("model", "mock/mock-small");
const concurrency = Number.parseInt(arg("concurrency", "10"), 10);
const seconds = Number.parseFloat(arg("seconds", "10"));
const stream = flag("stream");
const processes = Math.max(1, Number.parseInt(arg("processes", "1"), 10));
const label = arg("label", `c${concurrency}${stream ? "-stream" : ""}`);
const output = arg("output", "");
const quiet = flag("quiet");
const outputDir = fs.mkdtempSync("/tmp/lr-bench-workers-");

const workerPath = fileURLToPath(new URL("./benchmark-worker.mjs", import.meta.url));

if (!Number.isInteger(concurrency) || concurrency < 1) {
  throw new Error(`--concurrency must be a positive integer, got ${concurrency}`);
}
if (!(seconds > 0)) {
  throw new Error(`--seconds must be positive, got ${seconds}`);
}
if (processes > concurrency) {
  throw new Error(`--processes (${processes}) cannot exceed --concurrency (${concurrency})`);
}

// Split the connection budget across the workers, giving the remainder to
// the first ones so the total is exactly `concurrency`.
const base = Math.floor(concurrency / processes);
const extra = concurrency % processes;
const share = Array.from({ length: processes }, (_, i) => base + (i < extra ? 1 : 0));

const startedAt = performance.now();
const results = await Promise.all(share.map((connections, index) => new Promise((resolve, reject) => {
  const child = fork(workerPath, [], {
    env: {
      ...process.env,
      BENCH_URL: url.origin,
      BENCH_KEY: key,
      BENCH_MODEL: model,
      BENCH_CONNECTIONS: String(connections),
      BENCH_SECONDS: String(seconds),
      BENCH_STREAM: stream ? "1" : "0",
      BENCH_LABEL: `${label}-w${index}`,
      BENCH_OUT: `${outputDir}/worker-${index}.json`,
    },
    stdio: ["ignore", "pipe", "pipe", "ipc"],
  });
  let err = "";
  child.stderr.on("data", (chunk) => { err += chunk; });
  child.on("error", reject);
  child.on("exit", (code) => {
    if (code !== 0) {
      reject(new Error(`worker ${index} exited ${code}: ${err.slice(0, 400)}`));
      return;
    }
    try {
      const report = JSON.parse(fs.readFileSync(`${outputDir}/worker-${index}.json`, "utf8"));
      fs.rmSync(`${outputDir}/worker-${index}.json`, { force: true });
      resolve(report);
    } catch (error) {
      reject(new Error(`worker ${index} report unreadable: ${String(error)}`));
    }
  });
})));

const wallSeconds = (performance.now() - startedAt) / 1000;

const latencies = [];
const statuses = {};
const errorTypes = {};
let ok = 0;
let failed = 0;
for (const worker of results) {
  for (const value of worker.latencies) latencies.push(value);
  for (const [status, count] of Object.entries(worker.statuses)) {
    statuses[status] = (statuses[status] ?? 0) + count;
  }
  for (const [name, count] of Object.entries(worker.error_types)) {
    errorTypes[name] = (errorTypes[name] ?? 0) + count;
  }
  ok += worker.ok;
  failed += worker.errors;
}

latencies.sort((a, b) => a - b);
const percentile = (fraction) => {
  if (latencies.length === 0) return null;
  const index = Math.min(latencies.length - 1, Math.floor(fraction * latencies.length));
  return Number(latencies[index].toFixed(1));
};
const mean = latencies.length === 0
  ? null
  : Number((latencies.reduce((sum, value) => sum + value, 0) / latencies.length).toFixed(1));

const result = {
  label,
  url: url.origin,
  model,
  concurrency,
  processes,
  requested_seconds: seconds,
  wall_seconds: Number(wallSeconds.toFixed(3)),
  stream,
  requests: ok + failed,
  ok,
  errors: failed,
  rps: Number((ok / wallSeconds).toFixed(1)),
  mean_ms: mean,
  p50_ms: percentile(0.50),
  p90_ms: percentile(0.90),
  p95_ms: percentile(0.95),
  p99_ms: percentile(0.99),
  max_ms: latencies.length ? Number(latencies.at(-1).toFixed(1)) : null,
  statuses,
  error_types: errorTypes,
};

const line = JSON.stringify(result);
if (!quiet) console.log(line);
if (output) fs.appendFileSync(output, `${line}\n`);
