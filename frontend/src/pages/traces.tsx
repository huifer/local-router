import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ChevronRight,
  Download,
  GitBranch,
  MessageSquare,
  RefreshCw,
  Search,
} from "lucide-react";
import { toast } from "sonner";
import type {
  SessionSummary,
  TraceDetail,
  TraceNode,
  TraceSummary,
  TracesData,
} from "@/types";
import { downloadTraces, getTrace, getTraceSession, getTraceSessions, getTraces } from "@/api";
import { Badge, Button, Card, CardContent, EmptyState, Input, Tabs, TabsList, TabsTrigger } from "@/components/ui";
import { formatTime, formatTokens } from "@/components/shared/bits";
import { useT } from "@/i18n";

const periods = ["today", "7d", "30d", "all"] as const;
const views = ["sessions", "traces"] as const;
type View = (typeof views)[number];

export default function TracesPage() {
  const { t } = useT();
  const [view, setView] = useState<View>("sessions");
  const [period, setPeriod] = useState("7d");
  const [needle, setNeedle] = useState("");
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    // Both views are cheap, and the page shows one of them at a time.
    await Promise.all([getTraces(period), getTraceSessions(period)]);
  }, [period]);

  useEffect(() => {
    reload().catch((error: Error) => toast.error(error.message));
  }, [reload]);

  return (
    <div className="grid gap-5 pt-7">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <Tabs value={view} onValueChange={(value) => setView(value as View)}>
          <TabsList>
            <TabsTrigger value="sessions">{t("traces.sessions")}</TabsTrigger>
            <TabsTrigger value="traces">{t("traces.traces")}</TabsTrigger>
          </TabsList>
        </Tabs>
        <div className="flex items-center gap-3">
          <div className="flex gap-0.5">
            {periods.map((item) => (
              <button
                key={item}
                onClick={() => setPeriod(item)}
                className={`cursor-pointer rounded-sm border px-2.5 py-1.5 font-mono text-[10px] uppercase tracking-[0.08em] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
                  period === item
                    ? "border-primary/30 bg-primary/5 text-primary"
                    : "border-transparent text-muted-foreground hover:border-border hover:text-foreground"
                }`}
              >
                {t(`usage.${item}`)}
              </button>
            ))}
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => exportTraces("csv")}
            title={t("traces.exportCsv")}
          >
            <Download /> CSV
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => exportTraces("jsonl")}
            title={t("traces.exportJsonl")}
          >
            <Download /> JSONL
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              setBusy(true);
              reload()
                .then(() => toast.success(t("traces.refreshed")))
                .catch((error: Error) => toast.error(error.message))
                .finally(() => setBusy(false));
            }}
          >
            <RefreshCw className={busy ? "animate-spin" : undefined} /> {t("app.refresh")}
          </Button>
        </div>
      </div>

      {view === "sessions" ? (
        <SessionsView period={period} needle={needle} onNeedle={setNeedle} />
      ) : (
        <TracesView period={period} needle={needle} onNeedle={setNeedle} />
      )}
    </div>
  );
}

async function exportTraces(format: "csv" | "jsonl") {
  try {
    await downloadTraces(format);
  } catch (error) {
    toast.error((error as Error).message);
  }
}

/** The whole chain of one session: every request, every vendor call. */
function SessionsView({
  period,
  needle,
  onNeedle,
}: {
  period: string;
  needle: string;
  onNeedle: (value: string) => void;
}) {
  const { t } = useT();
  const [data, setData] = useState<{ sessions: SessionSummary[] } | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [detail, setDetail] = useState<{ tree: TraceNode | null } | null>(null);

  const reload = useCallback(async () => {
    setData(await getTraceSessions(period));
  }, [period]);

  useEffect(() => {
    reload().catch((error: Error) => toast.error(error.message));
  }, [reload]);

  const sessions = useMemo(() => {
    if (!data) return [];
    const wanted = needle.trim().toLowerCase();
    if (!wanted) return data.sessions;
    return data.sessions.filter((session) =>
      [session.session_id, session.title ?? "", ...session.agents, ...session.providers]
        .join(" ")
        .toLowerCase()
        .includes(wanted),
    );
  }, [data, needle]);

  const toggle = async (id: string) => {
    if (open === id) {
      setOpen(null);
      setDetail(null);
      return;
    }
    setOpen(id);
    setDetail(null);
    try {
      setDetail(await getTraceSession(id));
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="grid gap-3">
      <FilterBar
        needle={needle}
        onNeedle={onNeedle}
        count={data ? t("traces.sessionCount", { count: data.sessions.length }) : "—"}
      />
      {!data ? (
        <div className="h-64 animate-pulse border border-border bg-gradient-to-r from-card via-accent to-card bg-[length:200%_100%]" />
      ) : sessions.length === 0 ? (
        <Card>
          <EmptyState title={t("traces.empty")} detail={t("traces.emptyDetail")} />
        </Card>
      ) : (
        <div className="grid gap-2">
          {sessions.map((session) => (
            <Card key={session.session_id} className="overflow-hidden">
              <button
                type="button"
                onClick={() => toggle(session.session_id)}
                className="flex w-full cursor-pointer items-center gap-4 px-4 py-3 text-left transition-colors hover:bg-white/[0.02] max-[700px]:flex-wrap"
              >
                <ChevronRight
                  className={`size-4 shrink-0 text-muted-foreground transition-transform ${open === session.session_id ? "rotate-90" : ""}`}
                />
                <MessageSquare className="size-3.5 shrink-0 text-primary" />
                <span className="font-mono text-[11px] text-foreground">
                  {session.title || session.session_id}
                </span>
                <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
                  {session.agents.map((agent) => (
                    <Badge key={agent}>{agent}</Badge>
                  ))}
                  {session.providers.map((provider) => (
                    <Badge key={provider} variant="outline">
                      {provider}
                    </Badge>
                  ))}
                  {session.traces.length > 1 && (
                    <Badge variant="primary">{t("traces.tracesCount", { count: session.traces.length })}</Badge>
                  )}
                  {!session.ok && <Badge variant="destructive">{t("traces.failed")}</Badge>}
                </div>
                <div className="flex shrink-0 items-center gap-4 font-mono text-[11px] text-muted-foreground">
                  <span title={t("traces.requests")}>
                    {session.requests} {t("traces.req")}
                  </span>
                  <span title={t("traces.calls")}>
                    {session.upstream_calls} {t("traces.callsShort")}
                  </span>
                  <span className="usage-number">${session.cost_usd.toFixed(4)}</span>
                  <span className="hidden sm:inline">
                    {formatTokens(session.input_tokens)} / {formatTokens(session.output_tokens)}
                  </span>
                  <span className="hidden lg:inline">{formatTime(session.started_at)}</span>
                </div>
              </button>
              {open === session.session_id && (
                <div className="border-t border-border bg-background/40 px-4 py-3">
                  {!detail ? (
                    <div className="h-24 animate-pulse border border-border bg-gradient-to-r from-card via-accent to-card bg-[length:200%_100%]" />
                  ) : detail.tree ? (
                    <div className="grid gap-1 font-mono text-[11px]">
                      <TreeNodeRow node={detail.tree} depth={0} />
                    </div>
                  ) : (
                    <p className="text-[11px] text-muted-foreground">{t("traces.emptyDetail")}</p>
                  )}
                </div>
              )}
            </Card>
          ))}
        </div>
      )}
    </div>
  );
}

/** The traces: one task, and the vendor calls it made. */
function TracesView({
  period,
  needle,
  onNeedle,
}: {
  period: string;
  needle: string;
  onNeedle: (value: string) => void;
}) {
  const { t } = useT();
  const [data, setData] = useState<TracesData | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [detail, setDetail] = useState<TraceDetail | null>(null);

  const reload = useCallback(async () => {
    setData(await getTraces(period));
  }, [period]);

  useEffect(() => {
    reload().catch((error: Error) => toast.error(error.message));
  }, [reload]);

  const traces = useMemo(() => {
    if (!data) return [];
    const wanted = needle.trim().toLowerCase();
    if (!wanted) return data.traces;
    return data.traces.filter((trace) =>
      [
        trace.trace_id,
        ...trace.sessions,
        ...trace.agents,
        ...trace.providers,
        ...trace.models,
      ]
        .join(" ")
        .toLowerCase()
        .includes(wanted),
    );
  }, [data, needle]);

  const toggle = async (id: string) => {
    if (open === id) {
      setOpen(null);
      setDetail(null);
      return;
    }
    setOpen(id);
    setDetail(null);
    try {
      setDetail(await getTrace(id));
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="grid gap-3">
      <FilterBar
        needle={needle}
        onNeedle={onNeedle}
        count={data ? t("traces.count", { count: data.traces.length }) : "—"}
      />
      {!data ? (
        <div className="h-64 animate-pulse border border-border bg-gradient-to-r from-card via-accent to-card bg-[length:200%_100%]" />
      ) : traces.length === 0 ? (
        <Card>
          <EmptyState title={t("traces.empty")} detail={t("traces.emptyDetail")} />
        </Card>
      ) : (
        <div className="grid gap-2">
          {traces.map((trace) => (
            <TraceCard
              key={trace.trace_id}
              trace={trace}
              open={open === trace.trace_id}
              detail={open === trace.trace_id ? detail : null}
              onToggle={() => toggle(trace.trace_id)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function FilterBar({
  needle,
  onNeedle,
  count,
}: {
  needle: string;
  onNeedle: (value: string) => void;
  count: string;
}) {
  const { t } = useT();
  return (
    <div className="flex items-center gap-3">
      <div className="relative">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={needle}
          onChange={(event) => onNeedle(event.target.value)}
          placeholder={t("traces.filter")}
          className="h-8 w-64 pl-8 text-[12px]"
        />
      </div>
      <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-muted-foreground">
        {count}
      </span>
    </div>
  );
}

function TraceCard({
  trace,
  open,
  detail,
  onToggle,
}: {
  trace: TraceSummary;
  open: boolean;
  detail: TraceDetail | null;
  onToggle: () => void;
}) {
  const { t } = useT();
  return (
    <Card className="overflow-hidden">
      <button
        type="button"
        onClick={onToggle}
        className="flex w-full cursor-pointer items-center gap-4 px-4 py-3 text-left transition-colors hover:bg-white/[0.02] max-[700px]:flex-wrap"
      >
        <ChevronRight
          className={`size-4 shrink-0 text-muted-foreground transition-transform ${open ? "rotate-90" : ""}`}
        />
        <GitBranch className="size-3.5 shrink-0 text-primary" />
        <span className="font-mono text-[11px] text-foreground">{trace.trace_id}</span>
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
          {trace.sessions.slice(0, 1).map((session) => (
            <Badge key={session} variant="primary" title={session}>
              {session.length > 28 ? `${session.slice(0, 27)}…` : session}
            </Badge>
          ))}
          {trace.agents.map((agent) => (
            <Badge key={agent}>{agent}</Badge>
          ))}
          {trace.providers.map((provider) => (
            <Badge key={provider} variant="outline">
              {provider}
            </Badge>
          ))}
          {!trace.ok && <Badge variant="destructive">{t("traces.failed")}</Badge>}
        </div>
        <div className="flex shrink-0 items-center gap-4 font-mono text-[11px] text-muted-foreground">
          <span title={t("traces.requests")}>
            {trace.requests} {t("traces.req")}
          </span>
          <span title={t("traces.calls")}>
            {trace.upstream_calls} {t("traces.callsShort")}
          </span>
          <span className="usage-number">${trace.cost_usd.toFixed(4)}</span>
          <span className="hidden sm:inline">
            {formatTokens(trace.input_tokens)} / {formatTokens(trace.output_tokens)}
          </span>
          <span className="hidden lg:inline">{formatTime(trace.started_at)}</span>
        </div>
      </button>
      {open && (
        <div className="border-t border-border bg-background/40 px-4 py-3">
          {!detail ? (
            <div className="h-24 animate-pulse border border-border bg-gradient-to-r from-card via-accent to-card bg-[length:200%_100%]" />
          ) : (
            <div className="grid gap-1 font-mono text-[11px]">
              {detail.tree.map((node) => (
                <TreeNodeRow key={node.span.span_id} node={node} depth={0} />
              ))}
            </div>
          )}
        </div>
      )}
    </Card>
  );
}

function TreeNodeRow({ node, depth }: { node: TraceNode; depth: number }) {
  const { t } = useT();
  const span = node.span;
  const pad = { paddingLeft: `${depth * 16}px` };
  return (
    <>
      <div
        style={pad}
        className="flex flex-wrap items-center gap-2 border-l border-border/60 py-1 pl-3"
      >
        {span.kind === "session" && <GitBranch className="size-3 text-primary" />}
        <span className="text-foreground">
          {span.kind === "session"
            ? t("traces.session")
            : span.kind === "request"
              ? span.model
              : `${span.provider ?? "-"} / ${span.model}`}
        </span>
        {span.task_id && <Badge variant="primary">{span.task_id}</Badge>}
        {span.attempt && span.kind === "upstream" && (
          <span className="text-muted-foreground">#{span.attempt}</span>
        )}
        {span.preview && (
          <span className="max-w-[24rem] truncate text-muted-foreground" title={span.preview}>
            {span.preview}
          </span>
        )}
        <span className="text-muted-foreground">{(span.latency_ms / 1000).toFixed(2)}s</span>
        {span.kind === "upstream" && (
          <>
            <span
              className={
                span.skipped ? "text-muted-foreground" : span.ok ? "text-primary" : "text-destructive"
              }
            >
              {span.skipped
                ? t("traces.skipped")
                : span.ok
                  ? t("traces.ok")
                  : `${t("traces.failed")} ${span.status || ""}`}
            </span>
            {span.skipped && span.error && (
              <span className="text-muted-foreground">{span.error}</span>
            )}
            {span.upstream_request_id && (
              <span className="text-muted-foreground" title={t("traces.vendorId")}>
                id={span.upstream_request_id}
              </span>
            )}
          </>
        )}
        {span.cost_usd !== undefined && (
          <span className="usage-number">${span.cost_usd.toFixed(4)}</span>
        )}
      </div>
      {node.children.map((child) => (
        <TreeNodeRow key={child.span.span_id} node={child} depth={depth + 1} />
      ))}
    </>
  );
}
