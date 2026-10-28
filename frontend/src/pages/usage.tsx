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
import type { ProjectRow, QuotaAccount, UsageData, UsageTotals } from "@/types";
import { api, getProjects, getUsage, mergeProjects, updateProject } from "@/api";
import {
  Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Dialog,
  DialogContent, DialogFooter, DialogHeader, DialogTitle, EmptyState, Field, Input,
  Tabs, TabsList, TabsTrigger,
} from "@/components/ui";
import { SectionToolbar, StatusDot, formatTime, formatTokens } from "@/components/shared/bits";
import { useT } from "@/i18n";

const views = ["overview", "sessions", "projects", "context", "quota", "bills"] as const;
type View = (typeof views)[number];
const viewLabels: Record<View, string> = {
  overview: "Overview",
  sessions: "Sessions",
  projects: "Projects",
  context: "Context",
  quota: "Quota",
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
              <TabsTrigger key={item} value={item}>{t(`usage.${item}`)}</TabsTrigger>
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
          {view === "sessions" && <SessionsPanel usage={usage} />}
          {view === "projects" && <ProjectsPanel projects={projects} onChanged={reloadUsage} />}
          {view === "context" && <ContextPanel usage={usage} />}
          {view === "quota" && <QuotaPanel usage={usage} />}
          {view === "bills" && <BillsPanel usage={usage} />}
        </>
      )}
      {/* keep reload referenced for future wiring */}
      <span className="hidden">{typeof reload}</span>
    </div>
  );
}

const chartColors = ["#9ef0bf", "#5ec9a8", "#7ec9e8", "#a89ef0", "#f0d29e", "#f09e9e", "#9ef0e0", "#c9e87e"];

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
      <div className="grid grid-cols-5 border border-border bg-card max-[900px]:grid-cols-2">
        <Metric label={t("usage.requests")} value={totals.requests.toLocaleString()} />
        <Metric label={t("usage.failed")} value={totals.failed.toLocaleString()} danger={totals.failed > 0} />
        <Metric label={t("usage.inputTokens")} value={formatTokens(totals.input_tokens)} />
        <Metric label={t("usage.outputTokens")} value={formatTokens(totals.output_tokens)} />
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
                      {t("usage.requestsCount", { n: value.requests })} · {formatTokens(value.input_tokens)} in · {formatTokens(value.output_tokens)} out
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
                      {t("usage.requestsCount", { n: value.requests })} · {t("usage.output", { n: formatTokens(value.output_tokens) })}
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
  if (!points.length) {
    return <EmptyState title={t("usage.noRequests")} detail={t("usage.noRequestsDetail")} />;
  }
  const data = points.map((p) => ({ ...p, label: p.hour.slice(5, 13) }));
  return (
    <div className="h-40 px-3 pt-4">
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={data} margin={{ top: 4, right: 12, bottom: 0, left: -18 }}>
          <defs>
            <linearGradient id="timeline-fill" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="#9ef0bf" stopOpacity={0.35} />
              <stop offset="100%" stopColor="#9ef0bf" stopOpacity={0} />
            </linearGradient>
          </defs>
          <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
          <XAxis dataKey="label" tickLine={false} axisLine={false} interval="preserveStartEnd" minTickGap={24} />
          <YAxis tickLine={false} axisLine={false} allowDecimals={false} width={44} />
          <ReTooltip cursor={{ stroke: "var(--border)" }} />
          <Area type="monotone" dataKey="requests" stroke="#9ef0bf" strokeWidth={1.6} fill="url(#timeline-fill)" />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}

function DayChart({ data }: { data: [string, UsageTotals][] }) {
  const { t } = useT();
  if (!data.length) {
    return <EmptyState title={t("usage.noDaily")} detail={t("usage.noDailyDetail")} />;
  }
  const days = data.slice(-14).map(([day, v]) => ({ day: day.slice(5), requests: v.requests, cost: v.cost_usd }));
  return (
    <div className="h-44 px-3 pt-4">
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={days} margin={{ top: 4, right: 12, bottom: 0, left: -18 }} barCategoryGap="28%">
          <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
          <XAxis dataKey="day" tickLine={false} axisLine={false} />
          <YAxis yAxisId="req" tickLine={false} axisLine={false} allowDecimals={false} width={44} />
          <ReTooltip
            cursor={{ fill: "rgba(158,240,191,0.05)" }}
            formatter={(value, name) => (name === "requests" ? [value, "requests"] : [`$${Number(value).toFixed(4)}`, "cost"])}
          />
          <Bar yAxisId="req" dataKey="requests" fill="rgba(158,240,191,0.75)" radius={[2, 2, 0, 0]} maxBarSize={26} />
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
}

function ModelMix({ data }: { data: [string, UsageTotals][] }) {
  const { t } = useT();
  const models = data.filter(([, v]) => v.requests > 0).slice(0, 8);
  if (!models.length) {
    return <EmptyState title={t("usage.noMix")} detail={t("usage.noMixDetail")} />;
  }
  const total = models.reduce((sum, [, v]) => sum + v.requests, 0);
  return (
    <div className="flex items-center gap-5 p-5 max-[500px]:flex-col">
      <div className="size-36 shrink-0">
        <ResponsiveContainer width="100%" height="100%">
          <PieChart>
            <Pie data={models} dataKey="0" nameKey="0" innerRadius={36} outerRadius={60} strokeWidth={0} paddingAngle={1}>
              {models.map((entry, index) => (
                <Cell key={entry[0]} fill={chartColors[index % chartColors.length]} />
              ))}
            </Pie>
            <ReTooltip />
          </PieChart>
        </ResponsiveContainer>
      </div>
      <div className="grid min-w-0 flex-1 gap-1.5">
        {models.map(([name, v], index) => (
          <div key={name} className="flex items-center gap-2">
            <i className="size-2 shrink-0 rounded-sm" style={{ background: chartColors[index % chartColors.length] }} />
            <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground" title={name}>{name}</span>
            <strong className="font-mono text-[10px]">{Math.round((v.requests / total) * 100)}%</strong>
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

function BillsPanel({ usage }: { usage: UsageData }) {
  const { t } = useT();
  return (
    <div className="grid grid-cols-[1.2fr_0.8fr] gap-5 max-[1100px]:grid-cols-1">
      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("usage.billsKey")}</CardDescription>
          <CardTitle>{t("usage.perCallerKey")}</CardTitle>
        </CardHeader>
        <div className="px-5 py-2">
          {usage.by_key.length ? (
            usage.by_key.map(([name, value]) => (
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
