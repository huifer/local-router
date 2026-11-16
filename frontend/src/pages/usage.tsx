import { useCallback, useEffect, useMemo, useState } from "react";
import { useRouteLoaderData } from "react-router";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip as ReTooltip,
  XAxis,
  YAxis,
} from "recharts";
import { RefreshCw } from "lucide-react";
import { toast } from "sonner";
import type { AppData } from "@/App";
import type { CredentialRow, ProjectRow, QuotaAccount, UsageData, UsageRecord, UsageTotals } from "@/types";
import { api, getProjects, getUsage, mergeProjects, updateProject } from "@/api";
import {
  Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Dialog,
  DialogContent, DialogFooter, DialogHeader, DialogTitle, EmptyState, Field, Input,
  Tabs, TabsList, TabsTrigger,
} from "@/components/ui";
import { SectionToolbar, StatusDot, formatTime, formatTokens } from "@/components/shared/bits";
import { useT } from "@/i18n";

const views = ["overview", "tokens", "credentials", "sessions", "projects", "context", "quota", "bills"] as const;
type View = (typeof views)[number];
const viewLabels: Record<View, string> = {
  overview: "Overview",
  tokens: "Tokens",
  sessions: "Sessions",
  projects: "Projects",
  context: "Context",
  quota: "Quota",
  credentials: "Credentials",
  bills: "Bills",
};
const periods = [
  { id: "today", label: "Today" },
  { id: "7d", label: "7 days" },
  { id: "30d", label: "30 days" },
  { id: "all", label: "All" },
];

export default function UsagePage() {
  const { t } = useT();
  const { reload } = useRouteLoaderData("shell") as { data: AppData; reload: () => Promise<void> };
  const [view, setView] = useState<View>("overview");
  const [period, setPeriod] = useState("7d");
  const [usage, setUsage] = useState<UsageData | null>(null);
  const [projects, setProjects] = useState<ProjectRow[]>([]);

  const reloadUsage = useCallback(async () => {
    const [data, projectData] = await Promise.all([getUsage(period), getProjects(period)]);
    setUsage(data);
    setProjects(projectData.projects);
  }, [period]);

  useEffect(() => {
    reloadUsage().catch((error: Error) => toast.error(error.message));
  }, [reloadUsage]);

  return (
    <div className="grid gap-5 pt-7">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <Tabs value={view} onValueChange={(value) => setView(value as View)}>
          <TabsList>
            {views.map((item) => (
              <TabsTrigger key={item} value={item}>
                {t(item === "tokens" ? "usage.tokensTab" : `usage.${item}`)}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
        <div className="flex items-center gap-3">
          <div className="flex gap-0.5">
            {periods.map((item) => (
              <button
                key={item.id}
                onClick={() => setPeriod(item.id)}
                className={`cursor-pointer rounded-sm border px-2.5 py-1.5 font-mono text-[10px] uppercase tracking-[0.08em] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
                  period === item.id
                    ? "border-primary/30 bg-primary/5 text-primary"
                    : "border-transparent text-muted-foreground hover:border-border hover:text-foreground"
                }`}
              >
                {t(`usage.${item.id === "7d" ? "7d" : item.id === "30d" ? "30d" : item.id === "today" ? "today" : "all"}`)}
              </button>
            ))}
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => reloadUsage().then(() => toast.success(t("usage.refreshed"))).catch((e: Error) => toast.error(e.message))}
          >
            <RefreshCw /> {t("app.refresh")}
          </Button>
        </div>
      </div>

      {!usage ? (
        <div className="grid gap-4">
          {[120, 250, 250].map((height, index) => (
            <div
              key={index}
              className="animate-pulse border border-border bg-gradient-to-r from-card via-accent to-card bg-[length:200%_100%]"
              style={{ height }}
            />
          ))}
        </div>
      ) : (
        <>
          {view === "overview" && <UsageOverview usage={usage} />}
          {view === "tokens" && <TokensPanel usage={usage} />}
          {view === "sessions" && <SessionsPanel usage={usage} />}
          {view === "projects" && <ProjectsPanel projects={projects} onChanged={reloadUsage} />}
          {view === "context" && <ContextPanel usage={usage} />}
          {view === "quota" && <QuotaPanel usage={usage} />}
          {view === "credentials" && <CredentialsPanel usage={usage} />}
          {view === "bills" && <BillsPanel usage={usage} />}
        </>
      )}
      {/* keep reload referenced for future wiring */}
      <span className="hidden">{typeof reload}</span>
    </div>
  );
}

const chartColors = ["#9ef0bf", "#5ec9a8", "#7ec9e8", "#a89ef0", "#f0d29e", "#f09e9e", "#9ef0e0", "#c9e87e"];

const tokenColors = { input: "#7ec9e8", cache: "#a89ef0", output: "#9ef0bf" };

/** The billable volume. Reasoning is a break-out of output, never a 5th bucket. */
function billableTokens(t: UsageTotals): number {
  return (t.input_tokens ?? 0) + (t.output_tokens ?? 0)
    + (t.cache_read_tokens ?? 0) + (t.cache_write_tokens ?? 0);
}

function billableRecord(r: UsageRecord): number {
  return (r.input_tokens ?? 0) + (r.output_tokens ?? 0)
    + (r.cache_read_tokens ?? 0) + (r.cache_write_tokens ?? 0);
}

type TokenRank = "total" | "input" | "output" | "cache";

function rankValue(t: UsageTotals, rank: TokenRank): number {
  switch (rank) {
    case "input": return t.input_tokens ?? 0;
    case "output": return t.output_tokens ?? 0;
    case "cache": return (t.cache_read_tokens ?? 0) + (t.cache_write_tokens ?? 0);
    default: return billableTokens(t);
  }
}

type ChartMetric = "requests" | "tokens";

/** The same switch on both overview charts: counts or token volume. */
function MetricToggle({ value, onChange }: { value: ChartMetric; onChange: (m: ChartMetric) => void }) {
  const { t } = useT();
  return (
    <div className="flex gap-0.5">
      {(["requests", "tokens"] as const).map((metric) => (
        <button
          key={metric}
          onClick={() => onChange(metric)}
          className={`cursor-pointer rounded-sm border px-2 py-1 font-mono text-[9px] uppercase tracking-[0.08em] outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
            value === metric
              ? "border-primary/30 bg-primary/5 text-primary"
              : "border-transparent text-muted-foreground hover:border-border hover:text-foreground"
          }`}
        >
          {metric === "requests" ? t("usage.metricRequests") : t("usage.metricTokens")}
        </button>
      ))}
    </div>
  );
}

/** Three stacked token buckets, shared by the timeline and the daily chart. */
function TokenLegend({ t: translate }: { t: ReturnType<typeof useT>["t"] }) {
  return (
    <div className="chart-legend flex-wrap px-5 pb-1">
      <span><i className="dot" style={{ background: tokenColors.input }} />{translate("usage.inputLabel")}</span>
      <span><i className="dot" style={{ background: tokenColors.cache }} />{translate("usage.cacheLabel")}</span>
      <span><i className="dot" style={{ background: tokenColors.output }} />{translate("usage.outputLabel")}</span>
    </div>
  );
}

function UsageOverview({ usage }: { usage: UsageData }) {
  const { t } = useT();
  const totals = usage.totals;
  const perProvider = Object.entries(usage.by_provider_costs ?? {});
  const cacheRate =
    totals.input_tokens + totals.cache_read_tokens > 0
      ? totals.cache_read_tokens / (totals.input_tokens + totals.cache_read_tokens)
      : 0;
  return (
    <>
      <div className="flex items-center justify-between">
        <span />
        <Badge>{usage.storage === "sqlite" ? "SQLITE" : "JSONL"}</Badge>
      </div>
      <div className="grid grid-cols-3 border border-border bg-card max-[900px]:grid-cols-2">
        <Metric label={t("usage.requests")} value={totals.requests.toLocaleString()} />
        <Metric label={t("usage.failed")} value={totals.failed.toLocaleString()} danger={totals.failed > 0} />
        <Metric label={t("usage.totalTokens")} value={formatTokens(billableTokens(totals))} />
        <Metric label={t("usage.inputTokens")} value={formatTokens(totals.input_tokens)} />
        <Metric label={t("usage.outputTokens")} value={formatTokens(totals.output_tokens)} />
        <Metric label={t("usage.cacheRead")} value={formatTokens(totals.cache_read_tokens)} />
        <Metric label={t("usage.cacheHit")} value={`${Math.round(cacheRate * 100)}%`} />
        <Metric label={t("usage.cost")} value={`$${totals.cost_usd.toFixed(4)}`} />
        <Metric label={t("usage.credits")} value={totals.credits.toFixed(1)} />
      </div>
      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("usage.timeline")}</CardDescription>
          <CardTitle>{t("usage.requestsOverTime")}</CardTitle>
        </CardHeader>
        <TimelineChart points={usage.requests_timeline} />
      </Card>
      <div className="grid grid-cols-[1.2fr_0.8fr] gap-5 max-[1100px]:grid-cols-1">
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">{t("usage.byPlatform")}</CardDescription>
            <CardTitle>{t("usage.whatCost")}</CardTitle>
          </CardHeader>
          <div className="px-5 py-2">
            {perProvider.length ? (
              perProvider.map(([name, value]) => (
                <div key={name} className="flex items-center justify-between border-b border-border py-3.5 last:border-b-0">
                  <div>
                    <strong className="block font-mono text-[11px]">{name}</strong>
                    <span className="mt-1 block font-mono text-[10px] text-muted-foreground">
                      {t("usage.requestsCount", { n: value.requests })} · {formatTokens(billableTokens(value))} {t("usage.metricTokens")} · {formatTokens(value.input_tokens)} in · {formatTokens(value.output_tokens)} out
                      {(value.cache_read_tokens ?? 0) + (value.cache_write_tokens ?? 0) > 0
                        ? ` · ${formatTokens((value.cache_read_tokens ?? 0) + (value.cache_write_tokens ?? 0))} cache`
                        : ""}
                    </span>
                  </div>
                  <div className="text-right">
                    <span className="usage-number block text-[15px]">${value.cost_usd.toFixed(4)}</span>
                    <span className="usage-credits block">
                      {t("usage.creditsValue", { n: value.credits.toFixed(1) })}{value.unpriced > 0 ? ` · ${t("usage.unpriced", { n: value.unpriced })}` : ""}
                    </span>
                  </div>
                </div>
              ))
            ) : (
              <EmptyState title={t("usage.noPlatform")} detail={t("usage.noPlatformDetail")} />
            )}
          </div>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">{t("usage.byDay")}</CardDescription>
            <CardTitle>{t("usage.dailyActivity")}</CardTitle>
          </CardHeader>
          <DayChart data={usage.by_day} />
        </Card>
      </div>
      <div className="grid grid-cols-[1.2fr_0.8fr] gap-5 max-[1100px]:grid-cols-1">
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">{t("usage.byModel")}</CardDescription>
            <CardTitle>{t("usage.modelDistribution")}</CardTitle>
          </CardHeader>
          <div className="px-5 py-2">
            {usage.by_model.length ? (
              usage.by_model.map(([name, value]) => (
                <div key={name} className="flex items-center justify-between border-b border-border py-3.5 last:border-b-0">
                  <div>
                    <strong className="block font-mono text-[11px]">{name}</strong>
                    <span className="mt-1 block font-mono text-[10px] text-muted-foreground">
                      {t("usage.requestsCount", { n: value.requests })} · {formatTokens(billableTokens(value))} {t("usage.metricTokens")} · {t("usage.output", { n: formatTokens(value.output_tokens) })}
                    </span>
                  </div>
                  <span className="usage-number text-[15px]">{value.requests}</span>
                </div>
              ))
            ) : (
              <EmptyState title={t("usage.noModel")} detail={t("usage.noModelDetail")} />
            )}
          </div>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">{t("usage.share")}</CardDescription>
            <CardTitle>{t("usage.modelMix")}</CardTitle>
          </CardHeader>
          <ModelMix data={usage.by_model} />
        </Card>
      </div>
      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("usage.requestLog")}</CardDescription>
          <CardTitle>{t("usage.recentRequests")}</CardTitle>
        </CardHeader>
        {usage.recent.length ? (
          <div className="py-1">
            <div className="grid grid-cols-[10px_minmax(0,1fr)_150px_60px_42px] items-center gap-2.5 border-b border-border px-5 pb-2 max-[700px]:grid-cols-[8px_minmax(0,1fr)_auto]">
              <span />
              <span className="eyebrow">{t("usage.model")}</span>
              <span className="eyebrow hidden sm:block">{t("usage.tokens")}</span>
              <span className="eyebrow hidden sm:block">{t("usage.cost")}</span>
              <span />
            </div>
            {usage.recent.map((record, index) => (
              <div
                key={`${record.time}-${index}`}
                className="grid grid-cols-[10px_minmax(0,1fr)_150px_60px_42px] items-center gap-2.5 border-b border-border/70 px-5 py-3 last:border-b-0 max-[700px]:grid-cols-[8px_minmax(0,1fr)_auto]"
              >
                <span className={`size-1.5 rounded-full ${record.ok ? "bg-ring" : "bg-destructive"}`} />
                <div className="min-w-0">
                  <strong className="block truncate font-mono text-[11px]" title={record.error}>{record.model}</strong>
                  <span className="block truncate font-mono text-[10px] text-muted-foreground">
                    {record.provider} / {record.protocol}
                    {record.key_label || record.key_id ? ` · key:${record.key_label || record.key_id}` : ""}
                    {record.caller_key_label || record.caller_key_id ? ` · caller:${record.caller_key_label || record.caller_key_id}` : ""}
                    {record.agent ? ` · ${record.agent}` : ""}
                  </span>
                  <span className="block truncate font-mono text-[9px] text-muted-foreground/60 sm:hidden">
                    {formatTime(record.time)} · {record.latency_ms}ms · {formatTokens(record.input_tokens)}→{formatTokens(record.output_tokens)}
                    {typeof record.cost_usd === "number" ? ` · $${record.cost_usd.toFixed(4)}` : ""}
                    {typeof record.credits === "number" && record.credits > 0 ? ` · ${record.credits.toFixed(1)}cr` : ""}
                  </span>
                </div>
                <div className="hidden min-w-0 flex-col sm:flex">
                  <span className="font-mono text-[10px] text-muted-foreground">{formatTime(record.time)}</span>
                  <span className="font-mono text-[10px] text-foreground">
                    {formatTokens(record.input_tokens)} → {formatTokens(record.output_tokens)}
                  </span>
                </div>
                <div className="hidden flex-col items-end sm:flex">
                  {typeof record.cost_usd === "number" ? (
                    <span className="usage-number text-[11px]">${record.cost_usd.toFixed(4)}</span>
                  ) : (
                    <span className="font-mono text-[9px] text-muted-foreground/60">—</span>
                  )}
                  {typeof record.credits === "number" && record.credits > 0 ? (
                    <span className="usage-credits text-[9px]">{record.credits.toFixed(1)}cr</span>
                  ) : null}
                </div>
                <Badge variant={record.ok ? "primary" : "destructive"} className="justify-self-end">
                  {record.ok ? "OK" : "FAIL"}
                </Badge>
              </div>
            ))}
          </div>
        ) : (
          <EmptyState title={t("usage.noTraffic")} detail={t("usage.noTrafficDetail")} />
        )}
      </Card>
    </>
  );
}

function Metric({ label, value, danger = false }: { label: string; value: string; danger?: boolean }) {
  return (
    <div className={`min-h-24 border-r border-border p-4 last:border-r-0 max-[900px]:border-b ${danger ? "[&_strong]:text-destructive" : ""}`}>
      <span className="eyebrow">{label}</span>
      <strong className="mt-3.5 block font-mono text-2xl tracking-[-0.08em]">{value}</strong>
    </div>
  );
}

function TimelineChart({ points }: { points: UsageData["requests_timeline"] }) {
  const { t } = useT();
  const [metric, setMetric] = useState<ChartMetric>("tokens");
  if (!points.length) {
    return <EmptyState title={t("usage.noRequests")} detail={t("usage.noRequestsDetail")} />;
  }
  const data = points.map((p) => ({
    label: p.hour.slice(5, 13),
    requests: p.requests,
    input: p.input_tokens ?? 0,
    cache: (p.cache_read_tokens ?? 0) + (p.cache_write_tokens ?? 0),
    output: p.output_tokens ?? 0,
  }));
  return (
    <>
      <div className="flex justify-end px-4"><MetricToggle value={metric} onChange={setMetric} /></div>
      <div className="h-40 px-3 pt-2">
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={data} margin={{ top: 4, right: 12, bottom: 0, left: 6 }}>
            <defs>
              <linearGradient id="timeline-fill" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="#9ef0bf" stopOpacity={0.35} />
                <stop offset="100%" stopColor="#9ef0bf" stopOpacity={0} />
              </linearGradient>
            </defs>
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
            <XAxis dataKey="label" tickLine={false} axisLine={false} interval="preserveStartEnd" minTickGap={24} />
            <YAxis tickLine={false} axisLine={false} allowDecimals={false} width={52}
              tickFormatter={(value) => metric === "tokens" ? formatTokens(Number(value)) : String(value)} />
            <ReTooltip cursor={{ stroke: "var(--border)" }}
              formatter={(value, name) => metric === "tokens"
                ? [formatTokens(Number(value)), name]
                : [Number(value).toLocaleString(), t("usage.requests")]} />
            {metric === "requests" ? (
              <Area type="monotone" dataKey="requests" stroke="#9ef0bf" strokeWidth={1.6} fill="url(#timeline-fill)" />
            ) : (
              <>
                <Area type="monotone" dataKey="input" name={t("usage.inputLabel")} stackId="tokens" stroke={tokenColors.input} strokeWidth={1.2} fill={tokenColors.input} fillOpacity={0.3} />
                <Area type="monotone" dataKey="cache" name={t("usage.cacheLabel")} stackId="tokens" stroke={tokenColors.cache} strokeWidth={1.2} fill={tokenColors.cache} fillOpacity={0.3} />
                <Area type="monotone" dataKey="output" name={t("usage.outputLabel")} stackId="tokens" stroke={tokenColors.output} strokeWidth={1.2} fill={tokenColors.output} fillOpacity={0.3} />
              </>
            )}
          </AreaChart>
        </ResponsiveContainer>
      </div>
      {metric === "tokens" && <TokenLegend t={t} />}
    </>
  );
}

function DayChart({ data }: { data: [string, UsageTotals][] }) {
  const { t } = useT();
  const [metric, setMetric] = useState<ChartMetric>("tokens");
  if (!data.length) {
    return <EmptyState title={t("usage.noDaily")} detail={t("usage.noDailyDetail")} />;
  }
  const days = data.slice(-14).map(([day, v]) => ({
    day: day.slice(5),
    requests: v.requests,
    input: v.input_tokens ?? 0,
    cache: (v.cache_read_tokens ?? 0) + (v.cache_write_tokens ?? 0),
    output: v.output_tokens ?? 0,
  }));
  return (
    <>
      <div className="flex justify-end px-4"><MetricToggle value={metric} onChange={setMetric} /></div>
      <div className="h-44 px-3 pt-2">
        <ResponsiveContainer width="100%" height="100%">
          <BarChart data={days} margin={{ top: 4, right: 12, bottom: 0, left: 6 }} barCategoryGap="28%">
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
            <XAxis dataKey="day" tickLine={false} axisLine={false} />
            <YAxis tickLine={false} axisLine={false} allowDecimals={false} width={52}
              tickFormatter={(value) => metric === "tokens" ? formatTokens(Number(value)) : String(value)} />
            <ReTooltip
              cursor={{ fill: "rgba(158,240,191,0.05)" }}
              formatter={(value, name) => metric === "tokens"
                ? [formatTokens(Number(value)), name]
                : [Number(value).toLocaleString(), t("usage.requests")]}
            />
            {metric === "requests" ? (
              <Bar dataKey="requests" fill="rgba(158,240,191,0.75)" radius={[2, 2, 0, 0]} maxBarSize={26} />
            ) : (
              <>
                <Bar dataKey="input" name={t("usage.inputLabel")} stackId="tokens" fill={tokenColors.input} maxBarSize={26} />
                <Bar dataKey="cache" name={t("usage.cacheLabel")} stackId="tokens" fill={tokenColors.cache} maxBarSize={26} />
                <Bar dataKey="output" name={t("usage.outputLabel")} stackId="tokens" fill={tokenColors.output} radius={[2, 2, 0, 0]} maxBarSize={26} />
              </>
            )}
          </BarChart>
        </ResponsiveContainer>
      </div>
      {metric === "tokens" && <TokenLegend t={t} />}
    </>
  );
}

function ModelMix({ data }: { data: [string, UsageTotals][] }) {
  const { t } = useT();
  // Recharts needs a numeric `value`; passing the tuple array straight in
  // with dataKey="0" pointed it at the model *name* and rendered nothing.
  const models = data
    .filter(([, v]) => v.requests > 0)
    .slice(0, 8)
    .map(([name, v]) => ({ name, value: v.requests }));
  if (!models.length) {
    return <EmptyState title={t("usage.noMix")} detail={t("usage.noMixDetail")} />;
  }
  const total = models.reduce((sum, m) => sum + m.value, 0);
  return (
    <div className="flex items-center gap-5 p-5 max-[500px]:flex-col">
      <div className="size-36 shrink-0">
        <ResponsiveContainer width="100%" height="100%">
          <PieChart>
            <Pie data={models} dataKey="value" nameKey="name" innerRadius={36} outerRadius={60} strokeWidth={0} paddingAngle={1}>
              {models.map((entry, index) => (
                <Cell key={entry.name} fill={chartColors[index % chartColors.length]} />
              ))}
            </Pie>
            <ReTooltip />
          </PieChart>
        </ResponsiveContainer>
      </div>
      <div className="grid min-w-0 flex-1 gap-1.5">
        {models.map((m, index) => (
          <div key={m.name} className="flex items-center gap-2">
            <i className="size-2 shrink-0 rounded-sm" style={{ background: chartColors[index % chartColors.length] }} />
            <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground" title={m.name}>{m.name}</span>
            <strong className="font-mono text-[10px]">{Math.round((m.value / total) * 100)}%</strong>
          </div>
        ))}
      </div>
    </div>
  );
}

function SessionsPanel({ usage }: { usage: UsageData }) {
  const { t } = useT();
  const [query, setQuery] = useState("");
  const sessions = usage.by_session;
  const filtered = sessions.filter(
    (s) => s.title.toLowerCase().includes(query.toLowerCase()) || s.id.toLowerCase().includes(query.toLowerCase()),
  );
  return (
    <Card>
      <CardHeader className="flex-row items-center justify-between gap-4">
        <div>
          <CardDescription className="eyebrow">{t("usage.sessionsEyebrow")}</CardDescription>
          <CardTitle>{t("usage.conversations", { n: sessions.length.toString().padStart(2, "0") })}</CardTitle>
        </div>
        <div className="flex w-56 items-center gap-2 border border-border bg-secondary px-2.5 text-muted-foreground focus-within:border-ring">
          <span className="eyebrow">⌕</span>
          <Input className="h-8 border-0 bg-transparent px-0 focus-visible:ring-0" placeholder={t("usage.filterSessions")} value={query} onChange={(event) => setQuery(event.target.value)} />
        </div>
      </CardHeader>
      {filtered.length ? (
        <div className="overflow-auto">
          <div className="grid grid-cols-[minmax(220px,2fr)_100px_170px_160px_160px] gap-3 border-b border-border px-5 py-3">
            <span className="eyebrow">{t("usage.session")}</span><span className="eyebrow">{t("usage.requests")}</span><span className="eyebrow">{t("usage.tokens")}</span><span className="eyebrow">{t("usage.cost")}</span><span className="eyebrow">{t("usage.lastSeen")}</span>
          </div>
          {filtered.map((s) => (
            <div key={s.id} title={s.id} className="grid grid-cols-[minmax(220px,2fr)_100px_170px_160px_160px] items-center gap-3 border-b border-border/70 px-5 py-3.5 last:border-b-0 max-[900px]:grid-cols-2">
              <div className="model-name"><span className="model-bullet" /><strong>{s.title}</strong></div>
              <span className="font-mono text-[10px] text-muted-foreground">{s.requests}{s.failed ? ` · ${s.failed} failed` : ""}</span>
              <span className="font-mono text-[10px] text-muted-foreground">{formatTokens(s.input_tokens)} in / {formatTokens(s.output_tokens)} out</span>
              <span className="usage-number text-[13px]">${s.cost_usd.toFixed(4)}{s.credits > 0 ? ` · ${s.credits.toFixed(1)} cr` : ""}</span>
              <span className="font-mono text-[10px] text-muted-foreground">{s.last_seen ? formatTime(s.last_seen) : "—"}</span>
            </div>
          ))}
        </div>
      ) : (
        <EmptyState title={t("usage.noSessions")} detail={t("usage.noSessionsDetail")} />
      )}
    </Card>
  );
}

function ProjectsPanel({ projects, onChanged }: { projects: ProjectRow[]; onChanged: () => Promise<void> }) {
  const { t } = useT();
  const [editing, setEditing] = useState<ProjectRow | null>(null);
  const [mergeFrom, setMergeFrom] = useState<string | null>(null);

  const save = async (id: string, patch: { name?: string; color?: string; budget_tokens?: number | null; budget_cost?: number | null }) => {
    try {
      await updateProject(id, patch);
      setEditing(null);
      await onChanged();
      toast.success(t("usage.projectUpdated"));
    } catch (error) {
      toast.error((error as Error).message);
    }
  };
  const merge = async (from: string, into: string) => {
    if (!from || !into || from === into) return;
    if (!window.confirm(`Merge all usage of\n${from}\ninto\n${into}? This re-keys the history.`)) return;
    try {
      await mergeProjects(from, into);
      setMergeFrom(null);
      toast.success(t("usage.merged"));
      await onChanged();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <Card>
      <CardHeader className="flex-row items-center justify-between">
        <div>
          <CardDescription className="eyebrow">{t("usage.projectsEyebrow")}</CardDescription>
          <CardTitle>{t("usage.projectsCount", { n: projects.length.toString().padStart(2, "0") })}</CardTitle>
        </div>
        <Button variant="ghost" size="sm" onClick={() => onChanged().then(() => toast.success(t("usage.projectsRefreshed")))}>
          <RefreshCw /> {t("app.refresh")}
        </Button>
      </CardHeader>
      {projects.length ? (
        <div className="grid gap-3 p-4">
          {projects.map((project) => {
            const tokenBudget = project.budget_tokens ? Math.min(1, project.input_tokens / project.budget_tokens) : null;
            const costBudget = project.budget_cost ? Math.min(1, project.cost_usd / project.budget_cost) : null;
            return (
              <div key={project.id} className="border border-border bg-secondary/40 p-4">
                <div className="flex items-center gap-3">
                  <span className="size-2.5 shrink-0 rounded-full shadow-[0_0_0_3px_rgba(158,240,191,0.08)]" style={{ background: project.color }} />
                  <div className="min-w-0 flex-1">
                    <strong className="block text-[13px]">{project.name}</strong>
                    <span className="mt-0.5 block truncate font-mono text-[10px] text-muted-foreground/70">{project.id}</span>
                  </div>
                  <div className="flex items-center gap-2">
                    <Button variant="ghost" size="sm" onClick={() => setEditing(project)}>{t("usage.edit")}</Button>
                    {mergeFrom === null ? (
                      <Button variant="ghost" size="sm" onClick={() => setMergeFrom(project.id)}>{t("usage.merge")}</Button>
                    ) : (
                      <select
                        className="cursor-pointer border border-border bg-secondary px-2 py-1.5 text-[11px] text-muted-foreground outline-none"
                        defaultValue=""
                        onChange={(event) => {
                          const into = event.target.value;
                          setMergeFrom(null);
                          merge(project.id, into);
                        }}
                      >
                        <option value="" disabled>{t("usage.mergeInto")}</option>
                        {projects.filter((p) => p.id !== project.id).map((p) => (
                          <option key={p.id} value={p.id}>{p.name}</option>
                        ))}
                      </select>
                    )}
                  </div>
                </div>
                <div className="mt-3 flex flex-wrap items-baseline gap-3.5 border-t border-border pt-3 font-mono text-[10px] text-muted-foreground">
                  <span>{t("usage.requestsCount", { n: project.requests })}{project.failed ? ` · ${project.failed} ${t("usage.failed").toLowerCase()}` : ""}</span>
                  <span>{formatTokens(project.input_tokens)} in · {formatTokens(project.output_tokens)} out</span>
                  <span className="usage-number text-[13px]">${project.cost_usd.toFixed(4)}</span>
                  {project.credits > 0 && <span className="usage-credits">{project.credits.toFixed(1)} credits</span>}
                  <span className="ml-auto text-muted-foreground/60">{project.last_seen ? t("usage.last", { when: formatTime(project.last_seen) }) : t("usage.noRequestsInRange")}</span>
                </div>
                {(tokenBudget !== null || costBudget !== null) && (
                  <div className="mt-3 grid gap-1.5">
                    {tokenBudget !== null && (
                      <BudgetBar label="tokens" ratio={tokenBudget} />
                    )}
                    {costBudget !== null && (
                      <BudgetBar label="cost" ratio={costBudget} />
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ) : (
        <EmptyState title={t("usage.noProjects")} detail={t("usage.noProjectsDetail")} />
      )}
      {editing && (
        <ProjectEditModal project={editing} onClose={() => setEditing(null)} onSave={save} />
      )}
    </Card>
  );
}

function BudgetBar({ label, ratio }: { label: string; ratio: number }) {
  return (
    <div className="grid grid-cols-14 items-center gap-2.5 font-mono text-[9px] uppercase tracking-[0.08em] text-muted-foreground/70" style={{ gridTemplateColumns: "56px 1fr 44px" }}>
      <span>{label}</span>
      <div className="budget-track">
        <i className={cnFill(ratio)} style={{ width: `${ratio * 100}%` }} />
      </div>
      <span className="text-right">{Math.round(ratio * 100)}%</span>
    </div>
  );
}

function cnFill(ratio: number) {
  return ratio > 0.9 ? "budget-fill budget-hot" : "budget-fill";
}

function ProjectEditModal({ project, onClose, onSave }: {
  project: ProjectRow;
  onClose: () => void;
  onSave: (id: string, patch: { name?: string; color?: string; budget_tokens?: number | null; budget_cost?: number | null }) => void;
}) {
  const { t } = useT();
  const [name, setName] = useState(project.name);
  const [color, setColor] = useState(project.color);
  const [budgetTokens, setBudgetTokens] = useState(project.budget_tokens?.toString() ?? "");
  const [budgetCost, setBudgetCost] = useState(project.budget_cost?.toString() ?? "");
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("usage.editProject")}</DialogTitle>
        </DialogHeader>
        <div className="grid gap-4 px-5 py-5">
          <Field label={t("providers.name")}>
            <Input autoFocus value={name} placeholder={project.id} onChange={(event) => setName(event.target.value)} />
          </Field>
          <Field label={t("usage.color")} help={t("usage.colorHelp")}>
            <Input value={color} placeholder="#9ef0bf" onChange={(event) => setColor(event.target.value)} />
          </Field>
          <Field label={t("usage.tokenBudget")} help={t("usage.tokenBudgetHelp")}>
            <Input type="number" value={budgetTokens} onChange={(event) => setBudgetTokens(event.target.value)} />
          </Field>
          <Field label={t("usage.costBudget")}>
            <Input type="number" value={budgetCost} onChange={(event) => setBudgetCost(event.target.value)} />
          </Field>
        </div>
        <DialogFooter>
          <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>
          <Button
            variant="default"
            onClick={() =>
              onSave(project.id, {
                name,
                color,
                budget_tokens: budgetTokens.trim() ? Number(budgetTokens) : null,
                budget_cost: budgetCost.trim() ? Number(budgetCost) : null,
              })
            }
          >
            {t("usage.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ContextPanel({ usage }: { usage: UsageData }) {
  const { t } = useT();
  const rows = usage.context.by_provider;
  const ctxTotals = usage.context.totals;
  const total = ctxTotals.system_tokens + ctxTotals.tool_tokens + ctxTotals.file_tokens + ctxTotals.conversation_tokens || 1;
  return (
    <>
      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("usage.contextEyebrow")}</CardDescription>
          <CardTitle>{t("usage.whatFills")}</CardTitle>
        </CardHeader>
        {!usage.context.captured ? (
          <EmptyState title={t("usage.noContext")} detail={t("usage.noContextDetail")} />
        ) : (
          <div className="grid gap-3 p-5">
            <div className="flex h-3 overflow-hidden rounded-sm">
              <i className="dot-system h-full" style={{ width: `${(ctxTotals.system_tokens / total) * 100}%` }} />
              <i className="dot-tools h-full" style={{ width: `${(ctxTotals.tool_tokens / total) * 100}%` }} />
              <i className="dot-files h-full" style={{ width: `${(ctxTotals.file_tokens / total) * 100}%` }} />
              <i className="dot-conv h-full" style={{ width: `${(ctxTotals.conversation_tokens / total) * 100}%` }} />
            </div>
            <div className="chart-legend flex-wrap">
              <span><i className="dot dot-system" />{t("usage.system")} {formatTokens(ctxTotals.system_tokens)}</span>
              <span><i className="dot dot-tools" />{t("usage.tools")} {formatTokens(ctxTotals.tool_tokens)}</span>
              <span><i className="dot dot-files" />{t("usage.files")} {formatTokens(ctxTotals.file_tokens)}</span>
              <span><i className="dot dot-conv" />{t("usage.conversation")} {formatTokens(ctxTotals.conversation_tokens)}</span>
            </div>
          </div>
        )}
      </Card>
      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("usage.contextByProvider")}</CardDescription>
          <CardTitle>{t("usage.perPlatform")}</CardTitle>
        </CardHeader>
        {rows.length ? (
          <div className="overflow-auto">
            <div className="grid grid-cols-[minmax(160px,1fr)_90px_90px_minmax(240px,1.4fr)] gap-3 border-b border-border px-5 py-3">
              <span className="eyebrow">{t("usage.provider")}</span><span className="eyebrow">{t("usage.requests")}</span><span className="eyebrow">{t("usage.avgFill")}</span><span className="eyebrow">{t("usage.breakdown")}</span>
            </div>
            {rows.map((row) => (
              <div key={row.provider} className="grid grid-cols-[minmax(160px,1fr)_90px_90px_minmax(240px,1.4fr)] items-center gap-3 border-b border-border/70 px-5 py-3.5 last:border-b-0">
                <div className="model-name"><span className="model-bullet" /><strong>{row.provider}</strong></div>
                <span className="font-mono text-[10px] text-muted-foreground">{row.requests}</span>
                <span className="font-mono text-[10px] text-muted-foreground">{row.avg_fill > 0 ? `${Math.round(row.avg_fill * 100)}%` : "—"}</span>
                <span className="font-mono text-[10px] text-muted-foreground">
                  {formatTokens(row.system_tokens)} / {formatTokens(row.tool_tokens)} / {formatTokens(row.file_tokens)} / {formatTokens(row.conversation_tokens)}
                </span>
              </div>
            ))}
          </div>
        ) : (
          <EmptyState title={t("usage.noBreakdown")} detail={t("usage.noBreakdownDetail")} />
        )}
      </Card>
    </>
  );
}

function QuotaPanel({ usage }: { usage: UsageData }) {
  const { t } = useT();
  const accounts = usage.quota ?? [];
  // Rows that hold real windows first; the token-billed "unlimited" ones
  // and the error rows follow, so the numbers lead the panel.
  const sorted = [...accounts].sort((a, b) => {
    const rank = (q: QuotaAccount) => (q.windows.length ? 0 : q.error ? 2 : 1);
    return rank(a) - rank(b);
  });
  return (
    <>
      <div className="flex items-center justify-end">
        <Badge>{t("usage.quotaPerAccount")}</Badge>
      </div>
      {sorted.length ? (
        <div className="grid grid-cols-2 gap-3 max-[1100px]:grid-cols-1">
          {sorted.map((account, index) => (
            <Card key={`${account.provider}-${account.account ?? index}`}>
              <CardHeader className="pb-3">
                <CardDescription className="eyebrow">{account.name}</CardDescription>
                <CardTitle className="font-mono text-sm">{account.user || account.provider}</CardTitle>
              </CardHeader>
              <CardContent className="grid gap-3">
                {account.windows.length ? (
                  account.windows.map((w) => (
                    <div key={w.name} className="grid gap-2">
                      <div className="flex items-baseline justify-between">
                        <span className="font-mono text-[10px] text-muted-foreground">{w.name}</span>
                        <strong className="font-mono text-xs">{w.display}</strong>
                      </div>
                      {!w.unlimited && (
                        <div className="budget-track h-1.5">
                          <i
                            className={w.remaining <= 0 ? "budget-fill budget-hot" : "budget-fill"}
                            style={{ width: `${w.used + w.remaining > 0 ? (w.used / (w.used + w.remaining)) * 100 : 0}%` }}
                          />
                        </div>
                      )}
                      {w.resets_at && <div className="font-mono text-[9px] text-muted-foreground/60">{t("usage.resets", { when: formatTime(w.resets_at) })}</div>}
                    </div>
                  ))
                ) : (
                  <span className="block font-mono text-[10px] text-muted-foreground/70">
                    {account.error ?? (account.unlimited ? t("usage.tokenBilled") : t("usage.noWindows"))}
                  </span>
                )}
                <div className="flex items-center gap-2 border-t border-border pt-2 font-mono text-[9px] text-muted-foreground/70">
                  <span>{account.provider}</span>
                  {account.account ? <span>· {account.account}</span> : null}
                  {account.read_at ? <span className="ml-auto">{formatTime(account.read_at)}</span> : null}
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      ) : (
        <Card>
          <EmptyState title={t("usage.noAccounts")} detail={t("usage.noAccountsDetail")} />
        </Card>
      )}
    </>
  );
}

/** The dedicated token view: volume over time, where it went, and which
 * requests were the heaviest. Everything here comes from the same records
 * as the overview totals, so the numbers always add up. */
function TokensPanel({ usage }: { usage: UsageData }) {
  const { t } = useT();
  const [rank, setRank] = useState<TokenRank>("total");
  const totals = usage.totals;

  const hourly = useMemo(() => usage.requests_timeline.map((p) => ({
    label: p.hour.slice(5, 13),
    input: p.input_tokens ?? 0,
    cache: (p.cache_read_tokens ?? 0) + (p.cache_write_tokens ?? 0),
    output: p.output_tokens ?? 0,
  })), [usage.requests_timeline]);
  const daily = useMemo(() => usage.by_day.slice(-14).map(([day, v]) => ({
    day: day.slice(5),
    input: v.input_tokens ?? 0,
    cache: (v.cache_read_tokens ?? 0) + (v.cache_write_tokens ?? 0),
    output: v.output_tokens ?? 0,
  })), [usage.by_day]);
  const hasTokens = hourly.some((h) => h.input + h.cache + h.output > 0);
  const heaviest = useMemo(() => [...usage.recent]
    .filter((r) => r.ok && billableRecord(r) > 0)
    .sort((a, b) => billableRecord(b) - billableRecord(a))
    .slice(0, 20), [usage.recent]);
  const platforms: [string, UsageTotals][] = Object.entries(usage.by_provider_costs ?? {});

  return (
    <>
      <p className="font-mono text-[10px] leading-relaxed text-muted-foreground">{t("usage.billableNote")}</p>
      <div className="grid grid-cols-3 border border-border bg-card max-[900px]:grid-cols-2">
        <Metric label={t("usage.totalTokens")} value={formatTokens(billableTokens(totals))} />
        <Metric label={t("usage.inputLabel")} value={formatTokens(totals.input_tokens)} />
        <Metric label={t("usage.outputLabel")} value={formatTokens(totals.output_tokens)} />
        <Metric label={t("usage.cacheRead")} value={formatTokens(totals.cache_read_tokens)} />
        <Metric label={t("usage.cacheWrite")} value={formatTokens(totals.cache_write_tokens)} />
        <Metric label={t("usage.reasoning")} value={formatTokens(totals.reasoning_tokens)} />
      </div>

      {!hasTokens ? (
        <Card><EmptyState title={t("usage.noTokens")} detail={t("usage.noTokensDetail")} /></Card>
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardDescription className="eyebrow">{t("usage.tokensEyebrow")}</CardDescription>
              <CardTitle>{t("usage.tokensOverTime")}</CardTitle>
            </CardHeader>
            <div className="h-56 px-3 pt-4">
              <ResponsiveContainer width="100%" height="100%">
                <AreaChart data={hourly} margin={{ top: 4, right: 12, bottom: 0, left: 6 }}>
                  <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
                  <XAxis dataKey="label" tickLine={false} axisLine={false} interval="preserveStartEnd" minTickGap={24} />
                  <YAxis tickLine={false} axisLine={false} allowDecimals={false} width={52}
                    tickFormatter={(value) => formatTokens(Number(value))} />
                  <ReTooltip cursor={{ stroke: "var(--border)" }}
                    formatter={(value, name) => [formatTokens(Number(value)), name]} />
                  <Area type="monotone" dataKey="input" name={t("usage.inputLabel")} stackId="tokens" stroke={tokenColors.input} strokeWidth={1.2} fill={tokenColors.input} fillOpacity={0.3} />
                  <Area type="monotone" dataKey="cache" name={t("usage.cacheLabel")} stackId="tokens" stroke={tokenColors.cache} strokeWidth={1.2} fill={tokenColors.cache} fillOpacity={0.3} />
                  <Area type="monotone" dataKey="output" name={t("usage.outputLabel")} stackId="tokens" stroke={tokenColors.output} strokeWidth={1.2} fill={tokenColors.output} fillOpacity={0.3} />
                </AreaChart>
              </ResponsiveContainer>
            </div>
            <TokenLegend t={t} />
          </Card>

          <Card>
            <CardHeader>
              <CardDescription className="eyebrow">{t("usage.byDay")}</CardDescription>
              <CardTitle>{t("usage.dailyTokens")}</CardTitle>
            </CardHeader>
            <div className="h-52 px-3 pt-4">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={daily} margin={{ top: 4, right: 12, bottom: 0, left: 6 }} barCategoryGap="28%">
                  <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
                  <XAxis dataKey="day" tickLine={false} axisLine={false} />
                  <YAxis tickLine={false} axisLine={false} allowDecimals={false} width={52}
                    tickFormatter={(value) => formatTokens(Number(value))} />
                  <ReTooltip cursor={{ fill: "rgba(158,240,191,0.05)" }}
                    formatter={(value, name) => [formatTokens(Number(value)), name]} />
                  <Bar dataKey="input" name={t("usage.inputLabel")} stackId="tokens" fill={tokenColors.input} maxBarSize={28} />
                  <Bar dataKey="cache" name={t("usage.cacheLabel")} stackId="tokens" fill={tokenColors.cache} maxBarSize={28} />
                  <Bar dataKey="output" name={t("usage.outputLabel")} stackId="tokens" fill={tokenColors.output} radius={[2, 2, 0, 0]} maxBarSize={28} />
                </BarChart>
              </ResponsiveContainer>
            </div>
            <TokenLegend t={t} />
          </Card>
        </>
      )}

      <div className="flex flex-wrap items-center gap-3">
        <span className="eyebrow">{t("usage.rankBy")}</span>
        <div className="flex gap-0.5">
          {(["total", "input", "output", "cache"] as const).map((option) => (
            <button
              key={option}
              onClick={() => setRank(option)}
              className={`cursor-pointer rounded-sm border px-2.5 py-1.5 font-mono text-[10px] uppercase tracking-[0.08em] outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
                rank === option
                  ? "border-primary/30 bg-primary/5 text-primary"
                  : "border-transparent text-muted-foreground hover:border-border hover:text-foreground"
              }`}
            >
              {t(`usage.rank${option.charAt(0).toUpperCase()}${option.slice(1)}`)}
            </button>
          ))}
        </div>
      </div>
      <div className="grid grid-cols-2 gap-5 max-[1100px]:grid-cols-1">
        <TokenRankCard
          eyebrow={t("usage.byModel")}
          title={t("usage.tokensByModel")}
          rows={usage.by_model}
          rank={rank}
        />
        <TokenRankCard
          eyebrow={t("usage.byPlatform")}
          title={t("usage.tokensByPlatform")}
          rows={platforms}
          rank={rank}
        />
      </div>

      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("usage.tokenLog")}</CardDescription>
          <CardTitle>{t("usage.topTokenRequests")}</CardTitle>
        </CardHeader>
        {heaviest.length ? (
          <div className="overflow-auto">
            <div className="min-w-[720px]">
              <div className="grid grid-cols-[minmax(0,1.8fr)_90px_90px_90px_90px_100px_150px] items-center gap-2.5 border-b border-border px-5 py-3">
                <span className="eyebrow">{t("usage.model")}</span>
                <span className="eyebrow">{t("usage.inputLabel")}</span>
                <span className="eyebrow">{t("usage.cacheLabel")}</span>
                <span className="eyebrow">{t("usage.outputLabel")}</span>
                <span className="eyebrow">{t("usage.reasoning")}</span>
                <span className="eyebrow">{t("usage.totalTokens")}</span>
                <span className="eyebrow">{t("usage.lastSeen")}</span>
              </div>
              {heaviest.map((record, index) => (
                <div
                  key={`${record.time}-${index}`}
                  className="grid grid-cols-[minmax(0,1.8fr)_90px_90px_90px_90px_100px_150px] items-center gap-2.5 border-b border-border/70 px-5 py-2.5 last:border-b-0"
                >
                  <div className="min-w-0">
                    <strong className="block truncate font-mono text-[11px]" title={record.model}>{record.model}</strong>
                    <span className="block truncate font-mono text-[9px] text-muted-foreground/70">{record.provider}</span>
                  </div>
                  <span className="font-mono text-[10px] text-muted-foreground">{formatTokens(record.input_tokens)}</span>
                  <span className="font-mono text-[10px] text-muted-foreground">
                    {formatTokens((record.cache_read_tokens ?? 0) + (record.cache_write_tokens ?? 0))}
                  </span>
                  <span className="font-mono text-[10px] text-muted-foreground">{formatTokens(record.output_tokens)}</span>
                  <span className="font-mono text-[10px] text-muted-foreground/70">{formatTokens(record.reasoning_tokens ?? 0)}</span>
                  <strong className="usage-number text-[12px]">{formatTokens(billableRecord(record))}</strong>
                  <span className="font-mono text-[9px] text-muted-foreground/70">{formatTime(record.time)}</span>
                </div>
              ))}
            </div>
          </div>
        ) : (
          <EmptyState title={t("usage.noTokens")} detail={t("usage.noTokensDetail")} />
        )}
      </Card>

      <ProviderSpeedCard usage={usage} />

      <KeySpeedCard usage={usage} />
    </>
  );
}

const speedCols =
  "grid grid-cols-[minmax(0,1.6fr)_90px_70px_110px_90px_90px_90px_90px] items-center gap-2.5";

/** Per-provider requests, token volume and first-token latency in one table. */
function ProviderSpeedCard({ usage }: { usage: UsageData }) {
  const { t } = useT();
  const rows = useMemo(
    () =>
      usage.by_provider
        .filter(([, v]) => (v.ttft_samples ?? 0) > 0 || billableTokens(v) > 0)
        .sort((a, b) => billableTokens(b[1]) - billableTokens(a[1]))
        .slice(0, 12),
    [usage.by_provider],
  );
  const ms = (value?: number | null) => (value == null ? "—" : `${value} ms`);
  return (
    <Card>
      <CardHeader>
        <CardDescription className="eyebrow">{t("usage.perProviderSpeedEyebrow")}</CardDescription>
        <CardTitle>{t("usage.perProviderSpeed")}</CardTitle>
      </CardHeader>
      {rows.length ? (
        <div className="overflow-auto">
          <div className="min-w-[760px]">
            <div className={`${speedCols} border-b border-border px-5 py-3`}>
              <span className="eyebrow">{t("usage.providerCol")}</span>
              <span className="eyebrow">{t("usage.requests")}</span>
              <span className="eyebrow">{t("usage.failed")}</span>
              <span className="eyebrow">{t("usage.totalTokens")}</span>
              <span className="eyebrow">{t("usage.ttftSamples")}</span>
              <span className="eyebrow">{t("usage.ttftAvg")}</span>
              <span className="eyebrow">{t("usage.ttftP50")}</span>
              <span className="eyebrow">{t("usage.ttftP95")}</span>
            </div>
            {rows.map(([name, v]) => (
              <div
                key={name}
                className={`${speedCols} border-b border-border/70 px-5 py-2.5 last:border-b-0`}
              >
                <strong className="truncate font-mono text-[11px]" title={name}>{name}</strong>
                <span className="font-mono text-[10px] text-muted-foreground">{v.requests}</span>
                <span className="font-mono text-[10px] text-muted-foreground">{v.failed}</span>
                <strong className="usage-number text-[12px]">{formatTokens(billableTokens(v))}</strong>
                <span className="font-mono text-[10px] text-muted-foreground">{v.ttft_samples ?? 0}</span>
                <span className="font-mono text-[10px] text-muted-foreground">
                  {v.ttft_samples ? `${v.ttft_avg_ms} ms` : "—"}
                </span>
                <span className="font-mono text-[10px] text-muted-foreground">{ms(v.ttft_p50_ms)}</span>
                <span className="font-mono text-[10px] text-muted-foreground">{ms(v.ttft_p95_ms)}</span>
              </div>
            ))}
          </div>
        </div>
      ) : (
        <EmptyState title={t("usage.noTokens")} detail={t("usage.noTokensDetail")} />
      )}
      <p className="px-5 pb-4 pt-3 font-mono text-[10px] leading-relaxed text-muted-foreground">
        {t("usage.ttftNote")}
      </p>
    </Card>
  );
}

const keySpeedCols =
  "grid grid-cols-[minmax(0,1.4fr)_minmax(0,1.2fr)_70px_100px_90px_100px_80px_90px] items-center gap-2.5";

/** Per provider × upstream API key: volume and token rate in one table. */
function KeySpeedCard({ usage }: { usage: UsageData }) {
  const { t } = useT();
  const rows = useMemo(
    () =>
      (usage.by_key_speed ?? [])
        .slice()
        .sort((a, b) => b.total_tokens - a.total_tokens)
        .slice(0, 15),
    [usage.by_key_speed],
  );
  const rate = (value: number) => (value > 0 ? value.toFixed(1) : "—");
  return (
    <Card>
      <CardHeader>
        <CardDescription className="eyebrow">{t("usage.perKeySpeedEyebrow")}</CardDescription>
        <CardTitle>{t("usage.perKeySpeed")}</CardTitle>
      </CardHeader>
      {rows.length ? (
        <div className="overflow-auto">
          <div className="min-w-[760px]">
            <div className={`${keySpeedCols} border-b border-border px-5 py-3`}>
              <span className="eyebrow">{t("usage.providerCol")}</span>
              <span className="eyebrow">{t("usage.keyCol")}</span>
              <span className="eyebrow">{t("usage.requests")}</span>
              <span className="eyebrow">{t("usage.totalTokens")}</span>
              <span className="eyebrow">{t("usage.tokPerSec")}</span>
              <span className="eyebrow">{t("usage.outTokPerSec")}</span>
              <span className="eyebrow">{t("usage.ttftSamples")}</span>
              <span className="eyebrow">{t("usage.ttftAvg")}</span>
            </div>
            {rows.map((row) => (
              <div
                key={`${row.provider}/${row.key_id}`}
                className={`${keySpeedCols} border-b border-border/70 px-5 py-2.5 last:border-b-0`}
              >
                <strong className="truncate font-mono text-[11px]" title={row.provider}>{row.provider}</strong>
                <span className="truncate font-mono text-[10px] text-muted-foreground" title={row.key_label ?? row.key_id}>
                  {row.key_label || row.key_id}
                </span>
                <span className="font-mono text-[10px] text-muted-foreground">{row.requests}</span>
                <strong className="usage-number text-[12px]">{formatTokens(row.total_tokens)}</strong>
                <strong className="usage-number text-[12px]">{rate(row.tokens_per_sec)}</strong>
                <span className="font-mono text-[10px] text-muted-foreground">{rate(row.output_tokens_per_sec)}</span>
                <span className="font-mono text-[10px] text-muted-foreground">{row.ttft_samples}</span>
                <span className="font-mono text-[10px] text-muted-foreground">
                  {row.ttft_samples ? `${row.ttft_avg_ms} ms` : "—"}
                </span>
              </div>
            ))}
          </div>
        </div>
      ) : (
        <EmptyState title={t("usage.noTokens")} detail={t("usage.noTokensDetail")} />
      )}
      <p className="px-5 pb-4 pt-3 font-mono text-[10px] leading-relaxed text-muted-foreground">
        {t("usage.speedNote")}
      </p>
    </Card>
  );
}

const credCols =
  "grid grid-cols-[minmax(0,1.6fr)_76px_72px_120px_88px_84px_92px_100px_130px] items-center gap-2.5";

/** Per provider × API key / account: the complete token accounting.
 * Every configured credential has a row here, including idle ones, so no
 * provider is missing from the answer to "what did everyone spend?". */
function CredentialsPanel({ usage }: { usage: UsageData }) {
  const { t } = useT();
  const [query, setQuery] = useState("");
  const [showIdle, setShowIdle] = useState(true);
  const [sort, setSort] = useState<"tokens" | "requests" | "recent">("tokens");
  const rows = usage.credentials ?? [];

  const filtered = useMemo(
    () =>
      rows.filter((row) => {
        if (!showIdle && !row.seen) return false;
        const haystack = `${row.provider} ${row.provider_name ?? ""} ${row.credential} ${row.label}`.toLowerCase();
        return haystack.includes(query.trim().toLowerCase());
      }),
    [rows, query, showIdle],
  );

  const groups = useMemo(() => {
    const byProvider = new Map<string, CredentialRow[]>();
    for (const row of filtered) {
      const list = byProvider.get(row.provider) ?? [];
      list.push(row);
      byProvider.set(row.provider, list);
    }
    return [...byProvider.entries()]
      .map(([provider, list]) => {
        const sorted = [...list].sort((a, b) => {
          if (sort === "requests") return b.requests - a.requests;
          if (sort === "recent") return (b.last_seen ?? "").localeCompare(a.last_seen ?? "");
          return b.total_tokens - a.total_tokens;
        });
        return {
          provider,
          name: list[0]?.provider_name ?? provider,
          enabled: list.some((r) => r.provider_enabled ?? r.enabled),
          rows: sorted,
          totalTokens: list.reduce((n, r) => n + r.total_tokens, 0),
          requests: list.reduce((n, r) => n + r.requests, 0),
          failed: list.reduce((n, r) => n + r.failed, 0),
          cost: list.reduce((n, r) => n + r.cost_usd, 0),
          accounts: list.filter((r) => r.kind === "account").length,
          budget: list.find((r) => r.budget)?.budget ?? null,
          quotaAccounts: list.filter((r) => r.quota).length,
        };
      })
      .sort((a, b) => b.totalTokens - a.totalTokens);
  }, [filtered, sort]);

  const totals = useMemo(
    () => ({
      providers: groups.length,
      credentials: filtered.length,
      active: filtered.filter((r) => r.enabled).length,
      tokens: filtered.reduce((n, r) => n + r.total_tokens, 0),
      alerts: filtered.filter(
        (r) =>
          (r.budget && (r.budget.exhausted || r.budget.used_pct >= 0.8)) ||
          (r.quota && r.quota.remaining <= 0),
      ).length,
    }),
    [groups, filtered],
  );

  return (
    <>
      <div className="grid grid-cols-4 gap-3 max-[900px]:grid-cols-2">
        <Metric label={t("usage.credProviders")} value={String(totals.providers)} />
        <Metric label={t("usage.credKeys")} value={`${totals.credentials} · ${t("usage.credKeysDetail", { n: totals.active })}`} />
        <Metric label={t("usage.credTokens")} value={formatTokens(totals.tokens)} />
        <Metric label={t("usage.credAlerts")} value={String(totals.alerts)} danger={totals.alerts > 0} />
      </div>

      <Card>
        <CardHeader className="pb-3">
          <CardDescription className="eyebrow">{t("usage.credEyebrow")}</CardDescription>
          <CardTitle>{t("usage.credTitle")}</CardTitle>
        </CardHeader>
        <div className="flex flex-wrap items-center gap-2 px-5 pb-4">
          <Input
            aria-label={t("usage.credSearch")}
            placeholder={t("usage.credSearch")}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            className="h-8 w-64 font-mono text-[11px]"
          />
          <div className="flex gap-0.5">
            {(["tokens", "requests", "recent"] as const).map((key) => (
              <button
                key={key}
                onClick={() => setSort(key)}
                className={`cursor-pointer rounded-sm border px-2.5 py-1.5 font-mono text-[10px] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
                  sort === key
                    ? "border-primary/30 bg-primary/5 text-primary"
                    : "border-transparent text-muted-foreground hover:border-border hover:text-foreground"
                }`}
              >
                {t(`usage.credSort${key === "tokens" ? "Tokens" : key === "requests" ? "Requests" : "Recent"}`)}
              </button>
            ))}
          </div>
          <button
            onClick={() => setShowIdle((v) => !v)}
            className={`ml-auto cursor-pointer rounded-sm border px-2.5 py-1.5 font-mono text-[10px] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
              showIdle ? "border-primary/30 bg-primary/5 text-primary" : "border-border text-muted-foreground"
            }`}
          >
            {t("usage.credShowIdle")}
          </button>
        </div>
        <p className="px-5 pb-4 font-mono text-[10px] leading-relaxed text-muted-foreground">
          {t("usage.credNote")}
        </p>
      </Card>

      {groups.length ? groups.map((group) => (
        <Card key={group.provider}>
          <CardHeader className="pb-3">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <div className="min-w-0">
                <CardDescription className="eyebrow">
                  {group.enabled ? t("usage.credProviderActive") : t("usage.credProviderPaused")}
                  {" · "}{t("usage.credCount", { n: group.rows.length })}
                  {group.accounts ? ` · ${t("usage.credAccounts", { n: group.accounts })}` : ""}
                </CardDescription>
                <CardTitle className="truncate font-mono text-sm" title={group.provider}>{group.name}</CardTitle>
                <span className="font-mono text-[9px] text-muted-foreground">{group.provider}</span>
              </div>
              <div className="flex flex-wrap items-center gap-3 font-mono text-[10px] text-muted-foreground">
                <span>{t("usage.requests")} <strong className="usage-number">{group.requests}</strong></span>
                <span>{t("usage.failed")} {group.failed}</span>
                <span>{t("usage.totalTokens")} <strong className="usage-number">{formatTokens(group.totalTokens)}</strong></span>
                <span>{t("usage.cost")} <strong className="usage-number">${group.cost.toFixed(4)}</strong></span>
              </div>
            </div>
            {group.budget ? (
              <div className="grid gap-1.5 pt-3">
                <div className="flex items-baseline justify-between font-mono text-[10px]">
                  <span className="text-muted-foreground">{t("usage.credBudget")}</span>
                  <strong className={group.budget.exhausted ? "text-destructive" : "usage-number"}>
                    {formatTokens(group.budget.used)} / {formatTokens(group.budget.limit)}{" · "}
                    {t("usage.credRemaining")} {formatTokens(group.budget.remaining)}
                  </strong>
                </div>
                <div className="budget-track">
                  <i
                    className={group.budget.exhausted || group.budget.used_pct >= 0.8 ? "budget-fill budget-hot" : "budget-fill"}
                    style={{ width: `${Math.min(100, group.budget.used_pct * 100)}%` }}
                  />
                </div>
              </div>
            ) : null}
          </CardHeader>
          <div className="overflow-auto">
            <div className="min-w-[1080px]">
              <div className={`${credCols} border-y border-border bg-muted/20 px-5 py-2.5`}>
                <span className="eyebrow">{t("usage.credCol")}</span>
                <span className="eyebrow">{t("usage.requests")}</span>
                <span className="eyebrow">{t("usage.credSuccess")}</span>
                <span className="eyebrow">{t("usage.totalTokens")}</span>
                <span className="eyebrow">{t("usage.cost")}</span>
                <span className="eyebrow">{t("usage.tokPerSec")}</span>
                <span className="eyebrow">{t("usage.ttftP50")}</span>
                <span className="eyebrow">{t("usage.credLastSeen")}</span>
                <span className="eyebrow">{t("usage.credQuota")}</span>
              </div>
              {group.rows.map((row) => (
                <div
                  key={`${row.provider}/${row.credential}`}
                  className={`${credCols} border-b border-border/70 px-5 py-2.5 last:border-b-0`}
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-1.5">
                      <StatusDot active={row.enabled} />
                      <strong className="truncate font-mono text-[11px]" title={row.credential}>{row.label || row.credential}</strong>
                      <Badge variant={row.kind === "account" ? "primary" : "default"}>
                        {row.kind === "account" ? t("usage.credAccount") : t("usage.credKey")}
                      </Badge>
                      {!row.seen ? <Badge>{t("usage.credIdle")}</Badge> : null}
                    </div>
                    <span className="block truncate font-mono text-[9px] text-muted-foreground" title={row.credential}>
                      {row.credential}{" · "}{t("usage.credShare", { n: Math.round(row.share_of_provider * 100) })}
                    </span>
                  </div>
                  <span className="font-mono text-[10px] text-muted-foreground">
                    {row.requests}{row.failed ? <span className="text-destructive"> / −{row.failed}</span> : null}
                  </span>
                  <span className="font-mono text-[10px] text-muted-foreground">
                    {row.requests ? `${(row.success_rate * 100).toFixed(0)}%` : "—"}
                  </span>
                  <div className="grid gap-1">
                    <strong className="usage-number text-[12px]">{formatTokens(row.total_tokens)}</strong>
                    <TokenMix row={row} />
                    <span className="font-mono text-[9px] text-muted-foreground">
                      {t("usage.cacheLabel")} {formatTokens(row.cache_read_tokens + row.cache_write_tokens)}
                      {row.reasoning_tokens ? ` · ${t("usage.reasoning")} ${formatTokens(row.reasoning_tokens)}` : ""}
                    </span>
                  </div>
                  <span className="font-mono text-[10px] text-muted-foreground">
                    ${row.cost_usd.toFixed(4)}{row.credits ? <span className="block text-[9px]">{row.credits.toFixed(1)} cr</span> : null}
                  </span>
                  <span className="font-mono text-[10px] text-muted-foreground">
                    {row.tokens_per_sec > 0 ? row.tokens_per_sec.toFixed(1) : "—"}
                    {row.output_tokens_per_sec > 0 ? <span className="block text-[9px]">out {row.output_tokens_per_sec.toFixed(1)}</span> : null}
                  </span>
                  <span className="font-mono text-[10px] text-muted-foreground">
                    {row.ttft_p50_ms != null ? `${row.ttft_p50_ms} ms` : "—"}
                    <span className="block text-[9px]">n={row.ttft_samples}</span>
                  </span>
                  <span className="font-mono text-[9px] text-muted-foreground">
                    {row.last_seen ? formatTime(row.last_seen).slice(5, 16) : t("usage.credNever")}
                  </span>
                  <CredentialQuota row={row} />
                </div>
              ))}
            </div>
          </div>
        </Card>
      )) : (
        <Card><EmptyState title={t("usage.credEmpty")} detail={t("usage.credEmptyDetail")} /></Card>
      )}
    </>
  );
}

function TokenMix({ row }: { row: CredentialRow }) {
  const total = row.total_tokens || 1;
  const parts = [
    { value: row.input_tokens, color: tokenColors.input },
    { value: row.cache_read_tokens + row.cache_write_tokens, color: tokenColors.cache },
    { value: row.output_tokens, color: tokenColors.output },
  ];
  return (
    <div className="flex h-1 w-full overflow-hidden rounded-sm bg-muted">
      {parts.map((part, index) => (
        <span key={index} style={{ width: `${(part.value / total) * 100}%`, background: part.color }} />
      ))}
    </div>
  );
}

function CredentialQuota({ row }: { row: CredentialRow }) {
  const { t } = useT();
  const quota = row.quota;
  if (!quota) {
    return <span className="font-mono text-[9px] text-muted-foreground/70">{t("usage.tokenBilled")}</span>;
  }
  const capacity = quota.capacity || quota.used + quota.remaining || 1;
  const usedPct = Math.min(100, (quota.used / capacity) * 100);
  return (
    <div className="grid gap-1">
      <span className={`truncate font-mono text-[10px] ${quota.remaining <= 0 ? "text-destructive" : "usage-number"}`} title={quota.display}>
        {quota.display || `${quota.remaining} ${quota.unit}`}
      </span>
      <div className="budget-track">
        <i className={quota.remaining <= 0 ? "budget-fill budget-hot" : "budget-fill"} style={{ width: `${usedPct}%` }} />
      </div>
      <span className="truncate font-mono text-[9px] text-muted-foreground">
        {quota.window}{quota.resets_at ? ` · ${formatTime(quota.resets_at).slice(5, 16)}` : ""}
      </span>
    </div>
  );
}

function TokenRankCard({ eyebrow, title, rows, rank }: {
  eyebrow: string;
  title: string;
  rows: [string, UsageTotals][];
  rank: TokenRank;
}) {
  const { t } = useT();
  const sorted = rows
    .map(([name, v]) => ({ name, value: rankValue(v, rank), v }))
    .filter((row) => row.value > 0)
    .sort((a, b) => b.value - a.value)
    .slice(0, 12);
  const max = sorted[0]?.value ?? 1;
  return (
    <Card>
      <CardHeader>
        <CardDescription className="eyebrow">{eyebrow}</CardDescription>
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      {sorted.length ? (
        <div className="grid gap-3 px-5 py-3">
          {sorted.map((row) => (
            <div key={row.name} className="grid gap-1">
              <div className="flex items-baseline justify-between gap-3">
                <span className="min-w-0 truncate font-mono text-[10px]" title={row.name}>{row.name}</span>
                <strong className="usage-number shrink-0 text-[11px]">{formatTokens(row.value)}</strong>
              </div>
              <div className="budget-track h-1">
                <i className="budget-fill" style={{ width: `${(row.value / max) * 100}%` }} />
              </div>
              <span className="font-mono text-[9px] text-muted-foreground/60">
                {t("usage.inputLabel")} {formatTokens(row.v.input_tokens ?? 0)} · {t("usage.cacheLabel")} {formatTokens((row.v.cache_read_tokens ?? 0) + (row.v.cache_write_tokens ?? 0))} · {t("usage.outputLabel")} {formatTokens(row.v.output_tokens ?? 0)}
              </span>
            </div>
          ))}
        </div>
      ) : (
        <EmptyState title={t("usage.noTokens")} detail={t("usage.noTokensDetail")} />
      )}
    </Card>
  );
}

function BillsPanel({ usage }: { usage: UsageData }) {
  const { t } = useT();
  return (
    <div className="grid grid-cols-2 gap-5 max-[1100px]:grid-cols-1">
      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("usage.billsProviderKey")}</CardDescription>
          <CardTitle>{t("usage.perProviderKey")}</CardTitle>
        </CardHeader>
        <div className="px-5 py-2">
          {(usage.by_provider_key ?? usage.by_key).length ? (
            (usage.by_provider_key ?? usage.by_key).map(([name, value]) => (
              <div key={name} className="flex items-center justify-between border-b border-border py-3.5 last:border-b-0">
                <div>
                  <strong className="block font-mono text-[11px]">{name}</strong>
                  <span className="mt-1 block font-mono text-[10px] text-muted-foreground">
                    {t("usage.requestsCount", { n: value.requests })} · {formatTokens(value.input_tokens + value.output_tokens)} tokens
                  </span>
                </div>
                <span className="usage-number text-[15px]">${value.cost_usd.toFixed(4)}</span>
              </div>
            ))
          ) : (
            <EmptyState title={t("usage.noKeys")} detail={t("usage.noKeysDetail")} />
          )}
        </div>
      </Card>
      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("usage.billsCallerKey")}</CardDescription>
          <CardTitle>{t("usage.perCallerKey")}</CardTitle>
        </CardHeader>
        <div className="px-5 py-2">
          {usage.by_caller_key.length ? (
            usage.by_caller_key.map(([name, value]) => (
              <div key={name} className="flex items-center justify-between border-b border-border py-3.5 last:border-b-0">
                <div>
                  <strong className="block font-mono text-[11px]">{name}</strong>
                  <span className="mt-1 block font-mono text-[10px] text-muted-foreground">
                    {t("usage.requestsCount", { n: value.requests })} · {formatTokens(value.input_tokens + value.output_tokens)} tokens
                  </span>
                </div>
                <span className="usage-number text-[15px]">${value.cost_usd.toFixed(4)}</span>
              </div>
            ))
          ) : (
            <EmptyState title={t("usage.noKeys")} detail={t("usage.noKeysDetail")} />
          )}
        </div>
      </Card>
      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("usage.billsAgent")}</CardDescription>
          <CardTitle>{t("usage.perAgent")}</CardTitle>
        </CardHeader>
        <div className="px-5 py-2">
          {usage.by_agent.length ? (
            usage.by_agent.map(([name, value]) => (
              <div key={name} className="flex items-center justify-between border-b border-border py-3.5 last:border-b-0">
                <div>
                  <strong className="block font-mono text-[11px]">{name}</strong>
                  <span className="mt-1 block font-mono text-[10px] text-muted-foreground">
                    {t("usage.requestsCount", { n: value.requests })} · {formatTokens(value.input_tokens + value.output_tokens)} tokens
                  </span>
                </div>
                <span className="usage-number text-[15px]">${value.cost_usd.toFixed(4)}</span>
              </div>
            ))
          ) : (
            <EmptyState title={t("usage.noAgents")} detail={t("usage.noAgentsDetail")} />
          )}
        </div>
      </Card>
    </div>
  );
}
