import { useEffect, useState } from "react";
import { useNavigate, useRouteLoaderData } from "react-router";
import { RefreshCw, Terminal } from "lucide-react";
import { toast } from "sonner";
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  EmptyState,
} from "@/components/ui";
import { formatTime } from "@/components/shared/bits";
import type { AppData } from "@/App";
import type { UsageData, WorkBuddyStatus } from "@/types";
import { api, getUsage, getWorkBuddy } from "@/api";
import { useT } from "@/i18n";
import { maskName } from "@/lib/utils";

export default function OverviewPage() {
  const { t } = useT();
  const { data } = useRouteLoaderData("shell") as { data: AppData };
  const navigate = useNavigate();
  const [usage, setUsage] = useState<UsageData | null>(null);
  const [workbuddy, setWorkbuddy] = useState<WorkBuddyStatus | null>(null);

  const loadWorkBuddy = () =>
    getWorkBuddy().then(setWorkbuddy).catch(() => setWorkbuddy(null));

  useEffect(() => {
    getUsage().then(setUsage).catch(() => {});
  }, []);

  useEffect(() => {
    loadWorkBuddy();
  }, []);

  const settings = data.status.settings;
  const endpoint = `http://${settings.gateway_addr}:${settings.gateway_port}`;
  const recent = usage?.recent ?? [];

  return (
    <div className="grid gap-5 pt-7">
      <div className="grid grid-cols-[1.7fr_0.7fr_0.7fr] gap-3 max-[1100px]:grid-cols-1 max-[700px]:grid-cols-1">
        <div className="signal-card signal-wide">
          <span className="eyebrow">{t("overview.systemStatus")}</span>
          <div className="flex items-center gap-2.5">
            <span className="inline-block size-2 rounded-full bg-primary shadow-[0_0_0_4px_rgba(158,240,191,0.08)]" />
            <strong className="text-sm font-bold">{t("overview.operational")}</strong>
          </div>
          <div>
            <span className="text-[11px] text-muted-foreground">{t("overview.operationalDetail")}</span>
            <div className="mt-1.5 font-mono text-[11px] text-primary">{endpoint}</div>
          </div>
        </div>
        <div className="signal-card">
          <span className="eyebrow">{t("overview.providers")}</span>
          <div>
            <strong className="metric-value">{data.providers.length.toString().padStart(2, "0")}</strong>
            <span className="metric-detail mt-2 block">{t("overview.providersDetail")}</span>
          </div>
        </div>
        <div className="signal-card">
          <span className="eyebrow">{t("overview.models")}</span>
          <div>
            <strong className="metric-value">{data.models.length.toString().padStart(2, "0")}</strong>
            <span className="metric-detail mt-2 block">{t("overview.modelsDetail")}</span>
          </div>
        </div>
      </div>

      <div className="grid grid-cols-[1.2fr_0.8fr] gap-5 max-[1100px]:grid-cols-1">
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">{t("overview.quickAccess")}</CardDescription>
            <CardTitle>{t("overview.operatorSurface")}</CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            <div className="grid grid-cols-2 gap-px bg-border max-[700px]:grid-cols-1">
              {[
                { label: t("overview.manageProviders"), detail: t("overview.manageProvidersDetail"), to: "/providers", icon: "server" as const },
                { label: t("overview.configureRouting"), detail: t("overview.configureRoutingDetail"), to: "/groups", icon: "activity" as const },
                { label: t("overview.browseModels"), detail: t("overview.browseModelsDetail"), to: "/models", icon: "layers" as const },
                { label: t("overview.inspectUsage"), detail: t("overview.inspectUsageDetail"), to: "/usage", icon: "grid" as const },
              ].map((item) => (
                <button
                  key={item.to}
                  onClick={() => navigate(item.to)}
                  className="flex min-h-24 cursor-pointer items-center gap-3 bg-card px-4 py-4 text-left transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none"
                >
                  <OperatorIcon name={item.icon} />
                  <span className="min-w-0">
                    <strong className="block text-xs">{item.label}</strong>
                    <small className="mt-1 block text-[10px] text-muted-foreground">{item.detail}</small>
                  </span>
                  <span className="ml-auto text-[17px] text-muted-foreground">↗</span>
                </button>
              ))}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">{t("overview.connection")}</CardDescription>
            <CardTitle>{t("overview.agentEnv")}</CardTitle>
          </CardHeader>
          <div className="code-block">
            <div className="grid grid-cols-[180px_15px_1fr] max-[500px]:grid-cols-1">
              <span className="code-key">OPENAI_BASE_URL</span>
              <span>=</span>
              <span className="break-all">{endpoint}/v1</span>
            </div>
            <div className="grid grid-cols-[180px_15px_1fr] max-[500px]:grid-cols-1">
              <span className="code-key">ANTHROPIC_BASE_URL</span>
              <span>=</span>
              <span className="break-all">{endpoint}</span>
            </div>
            <div className="grid grid-cols-[180px_15px_1fr] max-[500px]:grid-cols-1">
              <span className="code-key">LOCAL_ROUTER_API_KEY</span>
              <span>=</span>
              <span className="break-all">{settings.default_caller_key}</span>
            </div>
          </div>
          <div className="flex items-center gap-2 border-t border-border px-4 py-3 font-mono text-[10px] text-muted-foreground/80">
            <Terminal className="size-3.5" /> {t("overview.envFootnote")}
          </div>
        </Card>
      </div>

      <div className="grid grid-cols-[1.2fr_0.8fr] gap-5 max-[1100px]:grid-cols-1">
        <Card>
          <CardHeader className="flex-row items-center justify-between">
            <div>
              <CardDescription className="eyebrow">{t("overview.liveFeed")}</CardDescription>
              <CardTitle>{t("overview.recentRequests")}</CardTitle>
            </div>
            <button
              className="cursor-pointer border-0 bg-transparent font-mono text-[11px] text-primary hover:underline focus-visible:outline-none"
              onClick={() => navigate("/usage")}
            >
              {t("overview.viewAll")} ↗
            </button>
          </CardHeader>
          {recent.length ? (
            <div className="py-1">
              {recent.slice(0, 5).map((record, index) => (
                <RequestRow key={`${record.time}-${index}`} record={record} />
              ))}
            </div>
          ) : (
            <EmptyState title={t("overview.noRequests")} detail={t("overview.noRequestsDetail")} />
          )}
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">{t("overview.routingPosture")}</CardDescription>
            <CardTitle>{t("overview.groupsAtGlance")}</CardTitle>
          </CardHeader>
          <div className="px-5 py-2">
            {data.groups.length ? (
              data.groups.slice(0, 4).map((group) => (
                <div key={group.id} className="flex items-center justify-between gap-3 border-b border-border py-3.5 last:border-b-0">
                  <div>
                    <strong className="block text-xs">{group.name}</strong>
                    <span className="mt-1 block font-mono text-[10px] text-muted-foreground">
                      {group.models.length} members / {group.routing}
                    </span>
                  </div>
                  <Badge variant="primary">group/{group.id}</Badge>
                </div>
              ))
            ) : (
              <EmptyState title={t("overview.noGroups")} detail={t("overview.noGroupsDetail")} />
            )}
          </div>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardDescription className="eyebrow">{t("overview.account")}</CardDescription>
          <CardTitle>{t("overview.workbuddySession")}</CardTitle>
        </CardHeader>
        <WorkBuddySection status={workbuddy} onReload={loadWorkBuddy} />
      </Card>
    </div>
  );
}

function OperatorIcon({ name }: { name: "server" | "activity" | "layers" | "grid" }) {
  // Kept as text glyphs rather than a second icon family: lucide covers the chrome.
  const glyphs = { server: "▣", activity: "∿", layers: "≡", grid: "⊞" };
  return (
    <span className="grid size-8 shrink-0 place-items-center border border-input text-primary" aria-hidden="true">
      {glyphs[name]}
    </span>
  );
}

function RequestRow({ record }: { record: UsageData["recent"][number] }) {
  return (
    <div className="grid grid-cols-[10px_minmax(0,1fr)_150px_60px_42px] items-center gap-2.5 border-b border-border/70 px-5 py-3 last:border-b-0 max-[700px]:grid-cols-[8px_minmax(0,1fr)_auto]">
      <span className={`size-1.5 rounded-full ${record.ok ? "bg-ring" : "bg-destructive"}`} />
      <div className="min-w-0">
        <strong className="block truncate font-mono text-[11px]">{record.model}</strong>
        <span className="block truncate font-mono text-[10px] text-muted-foreground">
          {record.provider} / {record.protocol}
        </span>
      </div>
      <span className="hidden font-mono text-[10px] text-muted-foreground max-[700px]:hidden sm:block">{formatTime(record.time)}</span>
      <span className="hidden font-mono text-[10px] text-muted-foreground sm:block">{record.latency_ms}ms</span>
      <Badge variant={record.ok ? "primary" : "destructive"} className="justify-self-end">
        {record.ok ? "OK" : "FAIL"}
      </Badge>
    </div>
  );
}

function WorkBuddySection({
  status,
  onReload,
}: {
  status: WorkBuddyStatus | null;
  onReload: () => Promise<void>;
}) {
  const { t } = useT();
  const [busy, setBusy] = useState(false);
  if (!status) {
    return <EmptyState title={t("overview.readingSession")} detail={t("overview.readingSessionDetail")} />;
  }
  if (!status.signed_in) {
    return (
      <EmptyState
        title={t("overview.notSignedIn")}
        detail={status.note ?? "Run local-router workbuddy login to sign in with a browser."}
      />
    );
  }
  const credits = status.credits;
  const checkin = credits?.checkin;
  // The account the console acts on: the one in use.
  const slot = status.account_name || "default";

  /** Claim today's check-in for the account in use. */
  const claim = async () => {
    setBusy(true);
    try {
      const result = await api<{
        granted?: boolean;
        already?: boolean;
        credit?: number | null;
        message?: string;
      }>(`/api/providers/workbuddy/accounts/${encodeURIComponent(slot)}/checkin`, {
        method: "POST",
        body: {},
      });
      if (result.granted) toast.success(t("providers.checkedIn", { n: result.credit ?? 0 }));
      else toast.success(result.message || t("providers.alreadyCheckedIn"));
      await onReload();
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-4 p-5">
      <div className="grid grid-cols-4 gap-3 max-[900px]:grid-cols-2">
        <div className="signal-card min-h-28">
          <span className="eyebrow">{t("overview.accountLabel")}</span>
          <div>
            <strong className="metric-value text-2xl">{maskName(status.account?.nickname) || "signed in"}</strong>
            <span className="metric-detail mt-1.5 block">{status.account?.type || "personal"}</span>
          </div>
        </div>
        <div className="signal-card min-h-28">
          <span className="eyebrow">{t("overview.creditsLeft")}</span>
          <div>
            <strong className="metric-value text-2xl">{credits?.remaining ?? 0}</strong>
            <span className="metric-detail mt-1.5 block">of {credits?.capacity ?? 0} this cycle</span>
          </div>
        </div>
        <div className="signal-card min-h-28">
          <span className="eyebrow">{t("overview.checkIn")}</span>
          <div>
            <strong className="metric-value text-2xl">{checkin ? (checkin.checked_in_today ? "DONE" : "TODO") : "--"}</strong>
            <span className="metric-detail mt-1.5 block">{checkin ? `streak ${checkin.streak_days} days` : "unknown"}</span>
          </div>
          <div className="mt-3 flex items-center gap-2">
            <Button
              variant="default"
              size="sm"
              onClick={claim}
              disabled={busy || checkin?.checked_in_today === true}
            >
              {busy
                ? "…"
                : checkin?.checked_in_today
                  ? t("providers.checkInDone")
                  : t("providers.checkInClaim")}
            </Button>
            <Button variant="ghost" size="sm" onClick={() => onReload()}>
              <RefreshCw /> {t("providers.quotaRefresh")}
            </Button>
          </div>
        </div>
        <div className="signal-card min-h-28">
          <span className="eyebrow">{t("overview.token")}</span>
          <div>
            <strong className="metric-value text-2xl">{status.token_expires_at ? formatTime(new Date(status.token_expires_at).toISOString()).slice(0, 10) : "--"}</strong>
            <span className="metric-detail mt-1.5 block">{t("overview.accessTokenExpiry")}</span>
          </div>
        </div>
      </div>
      {credits?.packages.length ? (
        <div className="overflow-auto">
          <div className="grid grid-cols-[minmax(200px,2fr)_110px_130px_110px] gap-3 border-b border-border px-1 pb-2.5">
            <span className="eyebrow">{t("overview.package")}</span><span className="eyebrow">{t("overview.remaining")}</span><span className="eyebrow">{t("overview.capacity")}</span><span className="eyebrow">{t("overview.used")}</span>
          </div>
          {credits.packages.map((item, index) => (
            <div key={`${item.package_name}-${index}`} className="grid grid-cols-[minmax(200px,2fr)_110px_130px_110px] items-center gap-3 border-b border-border/70 py-3 last:border-b-0">
              <div className="model-name"><span className="model-bullet" /><strong>{item.package_name}</strong></div>
              <span className="font-mono text-[10px] text-muted-foreground">{item.remaining}</span>
              <span className="font-mono text-[10px] text-muted-foreground">{item.capacity} {item.unit}</span>
              <span className="font-mono text-[10px] text-muted-foreground">{item.used}</span>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
