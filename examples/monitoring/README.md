# Monitoring example

A self-contained stack that shows what the gateway's `/metrics` can do:
Prometheus scraping the gateway, Grafana reading it, and a small dashboard.

```sh
docker compose -f examples/monitoring/compose.yaml up -d
```

- Prometheus: <http://localhost:9090>
- Grafana: <http://localhost:3000> (`admin` / `local-router` by default;
  override with `GRAFANA_ADMIN_USER` / `GRAFANA_ADMIN_PASSWORD`). Add a
  Prometheus data source pointing at `http://prometheus:9090`, then import
  `examples/monitoring/dashboard.json`.

The dashboard shows the live picture the console also shows, with history:
requests per minute, in-flight, cooldowns, token throughput, p99 latency,
requests per provider, and the 429 rate.

To monitor a **second** local-router (the multi-machine cascade), add a job
to `prometheus.yml` pointing at its address; each instance reports its own
series and the trace ids already travel downstream, so both trees line up.

When the gateway is bound to a non-loopback address, `/metrics` asks for the
same bearer key as `/api` — set `bearer_token` in the scrape config.
