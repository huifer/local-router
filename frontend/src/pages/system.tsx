import { useCallback, useEffect, useState } from "react";
import {
  Area,
  CartesianGrid,
  ComposedChart,
  Line,
  ResponsiveContainer,
  Tooltip as ReTooltip,
  XAxis,
  YAxis,
} from "recharts";
import type { SystemPoint, SystemSeries, SystemSnapshot, SystemSummary } from "@/types";
import { getSystem, getSystemSeries } from "@/api";
import { Card, CardDescription, CardHeader, CardTitle, EmptyState } from "@/components/ui";
import { Metric, PageIntro } from "@/components/shared/bits";
import { useT } from "@/i18n";

const RANGES = [
  { minutes: 15, label: "15m" },
  { minutes: 60, label: "1h" },
  { minutes: 360, label: "6h" },
  { minutes: 1440, label: "24h" },
] as const;

const cpuColor = "#9ef0bf";
const rssColor = "#7ec9e8";
const rateColor = "#9ef0bf";
const latencyColor = "#a89ef0";

export default function SystemPage() {
  const { t } = useT();
  const [snap, setSnap] = useState<SystemSnapshot | null>(null);
  const [series, setSeries] = useState<SystemSeries | null>(null);
  const [minutes, setMinutes] = useState<number>(60);

  // The live cards: the sampler only re-measures every interval, so a poll
  // twice as fast is plenty.
  useEffect(() => {
    const load = () => getSystem().then(setSnap).catch(() => {});
    load();
    const timer = setInterval(load, 5000);
    return () => clearInterval(timer);
  }, []);

  const loadSeries = useCallback(
    () => getSystemSeries(minutes).then(setSeries).catch(() => {}),
    [minutes],
  );
  useEffect(() => {
    loadSeries();
    const timer = setInterval(loadSeries, 15_000);
    return () => clearInterval(timer);
  }, [loadSeries]);

  const windows = new Map((snap?.windows ?? []).map((w) => [w.label, w]));
  const rate5m = windows.get("5m");
  const cores = snap?.cores ?? 1;

  return (
    <div className="grid gap-5 pt-7">
      <PageIntro>{t("system.intro", { interval: snap?.interval_secs ?? 10 })}</PageIntro>
      {snap && !snap.sampling && (
        <div className="border border-border bg-secondary px-4 py-3 text-[12px] text-muted-foreground">
          {t("system.samplingOff")}
        </div>
      )}

      <div className="grid grid-cols-4 gap-3 max-[1100px]:grid-cols-2 max-[700px]:grid-cols-2">
        <Metric
          label={t("system.cpu")}
          value={`${snap ? snap.cpu_pct.toFixed(1) : "0.0"}%`}
          detail={t("system.cpuDetail", { cores })}
          danger={!!snap && snap.cpu_pct > snap.cores * 90}
        />
        <Metric
          label={t("system.memory")}
          value={formatBytes(snap?.rss_bytes ?? 0)}
          detail={t("system.memoryDetail")}
        />
        <Metric
          label={t("system.rpm")}
          value={formatRate(rate5m ? rate5m.rps * 60 : 0)}
          detail={t("system.requestsDetail", {
            requests: rate5m?.requests ?? 0,
            errors: rate5m?.errors ?? 0,
          })}
        />
        <Metric
          label={t("system.inFlight")}
          value={String(snap?.in_flight ?? 0)}
          detail={t("system.inFlightDetail", { threads: snap?.threads ?? 0 })}
        />
        <Metric
          label={t("system.latencyP50")}
          value={formatMs(rate5m?.latency_p50_ms ?? null)}
        />
        <Metric
          label={t("system.latencyP95")}
          value={formatMs(rate5m?.latency_p95_ms ?? null)}
          danger={(rate5m?.latency_p95_ms ?? 0) > 30_000}
        />
        <Metric
          label={t("system.ttftP50")}
          value={formatMs(rate5m?.ttft_p50_ms ?? null)}
        />
        <Metric label={t("system.uptime")} value={formatDuration(snap?.uptime_secs ?? 0)} />
      </div>

      <Card>
        <CardHeader className="flex-row items-start justify-between gap-4 max-[700px]:flex-col">
          <div>
            <CardDescription className="eyebrow">{t("system.range")}</CardDescription>
            <CardTitle className="text-base">{t("system.cpuChart")}</CardTitle>
            <p className="mt-1.5 text-[11px] text-muted-foreground">{t("system.cpuChartDetail")}</p>
          </div>
          <RangeTabs minutes={minutes} onChange={setMinutes} />
        </CardHeader>
        <VitalsChart series={series} metric="resources" />
      </Card>

      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("system.range")}</CardDescription>
          <CardTitle className="text-base">{t("system.rateChart")}</CardTitle>
          <p className="mt-1.5 text-[11px] text-muted-foreground">{t("system.rateChartDetail")}</p>
        </CardHeader>
        <VitalsChart series={series} metric="rate" />
      </Card>

      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("system.windows")}</CardDescription>
          <CardTitle className="text-base">{t("system.windowsDetail")}</CardTitle>
        </CardHeader>
        <WindowTable windows={snap?.windows ?? []} />
      </Card>
    </div>
  );
}

function RangeTabs({ minutes, onChange }: { minutes: number; onChange: (m: number) => void }) {
  const { t } = useT();
  return (
    <div className="flex items-center gap-1 border border-border bg-secondary p-1">
      {RANGES.map((range) => (
        <button
          key={range.minutes}
          onClick={() => onChange(range.minutes)}
          className={`cursor-pointer border-0 px-2.5 py-1.5 font-mono text-[11px] transition-colors focus-visible:outline-none ${
            minutes === range.minutes
              ? "bg-primary/15 text-primary"
              : "bg-transparent text-muted-foreground hover:text-foreground"
          }`}
          aria-pressed={minutes === range.minutes}
          title={t("system.range")}
        >
          {range.label}
        </button>
      ))}
    </div>
  );
}

function VitalsChart({ series, metric }: { series: SystemSeries | null; metric: "resources" | "rate" }) {
  const { t } = useT();
  const points = series?.points ?? [];
  if (!points.length) {
    return <EmptyState title={t("system.noSeries")} detail={t("system.noSeriesDetail")} />;
  }
  const data = points.map((p) => ({
    ...p,
    label: axisTime(p.t, series?.minutes ?? 60),
    rss_mb: Math.round(p.rss_bytes / (1024 * 1024)),
    p50: p.latency_p50_ms ?? null,
    p95: p.latency_p95_ms ?? null,
    ttft: p.ttft_p50_ms ?? null,
  }));
  const height = metric === "resources" ? "h-56" : "h-56";
  return (
    <div className={`${height} px-3 pb-3`}>
      <ResponsiveContainer width="100%" height="100%">
        <ComposedChart data={data} margin={{ top: 4, right: 8, bottom: 0, left: 6 }}>
          <defs>
            <linearGradient id={`sys-${metric}-fill`} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={metric === "resources" ? cpuColor : rateColor} stopOpacity={0.32} />
              <stop offset="100%" stopColor={metric === "resources" ? cpuColor : rateColor} stopOpacity={0} />
            </linearGradient>
          </defs>
          <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
          <XAxis dataKey="label" tickLine={false} axisLine={false} interval="preserveStartEnd" minTickGap={40} />
          <YAxis
            yAxisId="left"
            tickLine={false}
            axisLine={false}
            width={54}
            tickFormatter={(value: number) =>
              metric === "resources"
                ? `${tickNumber(Number(value))}%`
                : formatRate(Number(value))
            }
          />
          <YAxis
            yAxisId="right"
            orientation="right"
            tickLine={false}
            axisLine={false}
            width={54}
            tickFormatter={(value: number) =>
              metric === "resources" ? `${Math.round(Number(value))}` : `${Math.round(Number(value))}ms`
            }
          />
          <ReTooltip
            cursor={{ stroke: "var(--border)" }}
            content={<VitalsTooltip metric={metric} />}
          />
          {metric === "resources" ? (
            <>
              <Area
                yAxisId="left"
                type="monotone"
                dataKey="cpu_pct"
                name={t("system.cpu")}
                stroke={cpuColor}
                strokeWidth={1.6}
                fill={`url(#sys-resources-fill)`}
                isAnimationActive={false}
              />
              <Line
                yAxisId="right"
                type="monotone"
                dataKey="rss_mb"
                name={t("system.memory")}
                stroke={rssColor}
                strokeWidth={1.4}
                dot={false}
                isAnimationActive={false}
              />
            </>
          ) : (
            <>
              <Area
                yAxisId="left"
                type="monotone"
                dataKey="rps"
                name={t("system.colRps")}
                stroke={rateColor}
                strokeWidth={1.6}
                fill={`url(#sys-rate-fill)`}
                isAnimationActive={false}
              />
              <Line
                yAxisId="right"
                type="monotone"
                dataKey="p95"
                name={t("system.colP95")}
                stroke={latencyColor}
                strokeWidth={1.4}
                dot={false}
                connectNulls
                isAnimationActive={false}
              />
              <Line
                yAxisId="right"
                type="monotone"
                dataKey="ttft"
                name={t("system.colTtft")}
                stroke={rssColor}
                strokeWidth={1.2}
                strokeDasharray="4 3"
                dot={false}
                connectNulls
                isAnimationActive={false}
              />
            </>
          )}
        </ComposedChart>
      </ResponsiveContainer>
    </div>
  );
}

function VitalsTooltip({ active, payload, label, metric }: {
  active?: boolean;
  payload?: Array<{
    name?: string;
    value?: number | string | null;
    dataKey?: string;
    color?: string;
    payload?: SystemPoint;
  }>;
  label?: string;
  metric: "resources" | "rate";
}) {
  const { t } = useT();
  if (!active || !payload?.length) return null;
  const rows = payload.map((entry) => {
    const key = entry.dataKey ?? "";
    let value = String(entry.value ?? "—");
    if (key === "cpu_pct") value = `${Number(entry.value).toFixed(1)}%`;
    else if (key === "rss_mb") value = `${Number(entry.value).toLocaleString()} MB`;
    else if (key === "rps") value = formatRate(Number(entry.value));
    else if (key === "p95" || key === "ttft") value = formatMs(Number(entry.value));
    return { name: entry.name ?? key, value, color: entry.color };
  });
  const buckets = metric === "resources" ? null : payload[0]?.payload;
  return (
    <div className="border border-border bg-popover px-3 py-2 font-mono text-[11px] shadow-sm">
      <div className="mb-1 text-muted-foreground">{label}</div>
      {rows.map((row) => (
        <div key={row.name} className="flex items-center gap-2">
          <span className="size-1.5" style={{ background: row.color }} />
          <span>{row.name}</span>
          <span className="ml-auto">{row.value}</span>
        </div>
      ))}
      {buckets && (buckets.requests > 0 || buckets.errors > 0) && (
        <div className="mt-1 border-t border-border pt-1 text-[10px] text-muted-foreground">
          {t("system.requestsDetail", { requests: buckets.requests, errors: buckets.errors }).replace(" (5m)", "")}
        </div>
      )}
    </div>
  );
}

function WindowTable({ windows }: { windows: SystemSummary[] }) {
  const { t } = useT();
  if (!windows.length) {
    return <EmptyState title={t("system.noSeries")} detail={t("system.noSeriesDetail")} />;
  }
  return (
    <div className="py-1">
      <div className="grid grid-cols-[80px_1fr_1fr_1fr_1fr_1fr] gap-2 border-b border-border px-5 py-2.5 font-mono text-[10px] uppercase tracking-[0.06em] text-muted-foreground max-[700px]:grid-cols-[70px_repeat(6,1fr)] max-[700px]:overflow-x-auto">
        <span>{t("system.colWindow")}</span>
        <span>{t("system.colRequests")}</span>
        <span>{t("system.colRps")}</span>
        <span>{t("system.colP50")}</span>
        <span>{t("system.colP95")}</span>
        <span>{t("system.colTtft")}</span>
      </div>
      {windows.map((w) => (
        <div
          key={w.label}
          className="grid grid-cols-[80px_1fr_1fr_1fr_1fr_1fr] gap-2 border-b border-border/70 px-5 py-3 font-mono text-[11px] last:border-b-0 max-[700px]:grid-cols-[70px_repeat(6,1fr)] max-[700px]:overflow-x-auto"
        >
          <span className="text-primary">{w.label}</span>
          <span>
            {w.requests.toLocaleString()}
            {w.errors > 0 && <span className="ml-1.5 text-destructive">({w.errors} ✕)</span>}
          </span>
          <span>{formatRate(w.rps)}</span>
          <span>{formatMs(w.latency_p50_ms)}</span>
          <span>{formatMs(w.latency_p95_ms)}</span>
          <span>{formatMs(w.ttft_p50_ms)}</span>
        </div>
      ))}
    </div>
  );
}

// ---- formatting -----------------------------------------------------------

/** Tick labels that stay readable when the gateway is idle (0.3% is not 0%). */
function tickNumber(value: number): string {
  if (Math.abs(value) >= 10) return value.toFixed(0);
  if (Math.abs(value) >= 1) return value.toFixed(1);
  return value.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
}

function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${bytes} B`;
}

function formatRate(rps: number): string {
  if (rps >= 1000) return `${(rps / 1000).toFixed(1)}k`;
  if (rps >= 10) return rps.toFixed(0);
  return rps.toFixed(1);
}

function formatMs(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  if (value >= 1000) return `${(value / 1000).toFixed(value >= 10_000 ? 0 : 1)}s`;
  return `${Math.round(value)}ms`;
}

function formatDuration(seconds: number): string {
  if (seconds >= 86400) {
    const days = Math.floor(seconds / 86400);
    const hours = Math.floor((seconds % 86400) / 3600);
    return `${days}d ${hours}h`;
  }
  if (seconds >= 3600) {
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    return `${hours}h ${minutes}m`;
  }
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m ${Math.floor(seconds % 60)}s`;
}

/** HH:MM for short ranges, MM-DD HH:MM for a day. */
function axisTime(t: number, minutes: number): string {
  const date = new Date(t);
  const hh = `${date.getHours()}`.padStart(2, "0");
  const mm = `${date.getMinutes()}`.padStart(2, "0");
  if (minutes > 180) {
    return `${date.getMonth() + 1}-${date.getDate()} ${hh}:${mm}`;
  }
  return `${hh}:${mm}`;
}
