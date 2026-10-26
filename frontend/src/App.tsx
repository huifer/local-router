import { useCallback, useEffect, useMemo, useState } from "react";
import { api, getCallerKeys, getGroups, getModels, getPresets, getProjects, getProviders, getStatus, getUsage, getWorkBuddy, mergeProjects, saveSettings, updateProject } from "./api";
import type { CallerKey, Group, ModelItem, Preset, ProjectRow, Provider, SessionRow, Settings, Status, TimelinePoint, UsageData, UsageTotals, WorkBuddyStatus } from "./types";
import { Button, EmptyState, formatTime, formatTokens, Icon, Panel, StatusDot } from "./components";
import "./styles.css";

type Tab = "overview" | "providers" | "models" | "groups" | "usage" | "settings";

const tabs: { id: Tab; label: string; icon: string }[] = [
  { id: "overview", label: "Overview", icon: "grid" },
  { id: "providers", label: "Providers", icon: "server" },
  { id: "models", label: "Models", icon: "layers" },
  { id: "groups", label: "Routing groups", icon: "activity" },
  { id: "usage", label: "Usage", icon: "activity" },
  { id: "settings", label: "Settings", icon: "settings" },
];

interface AppData { status: Status; providers: Provider[]; groups: Group[]; models: ModelItem[]; }

export default function App() {
  const [tab, setTab] = useState<Tab>("overview");
  const [data, setData] = useState<AppData | null>(null);
  const [usage, setUsage] = useState<UsageData | null>(null);
  const [callerKeys, setCallerKeys] = useState<CallerKey[]>([]);
  const [workbuddy, setWorkbuddy] = useState<WorkBuddyStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [notice, setNotice] = useState<{ text: string; error?: boolean } | null>(null);

  const notify = useCallback((text: string, error = false) => {
    setNotice({ text, error });
    window.setTimeout(() => setNotice(null), 3000);
  }, []);

  const reload = useCallback(async () => {
    const [status, providers, groups, models] = await Promise.all([getStatus(), getProviders(), getGroups(), getModels()]);
    setData({ status: { ...status, settings: status.settings }, providers: providers.providers, groups: groups.groups, models: models.models });
  }, []);

  useEffect(() => {
    reload().catch((error: Error) => notify(error.message, true)).finally(() => setLoading(false));
  }, [reload, notify]);

  useEffect(() => {
    if (tab !== "usage") return;
    getUsage().then(setUsage).catch((error: Error) => notify(error.message, true));
  }, [tab, notify]);

  useEffect(() => {
    getWorkBuddy().then(setWorkbuddy).catch(() => setWorkbuddy(null));
  }, [tab, data]);

  const currentTab = tabs.find((item) => item.id === tab)!;
  if (loading || !data) return <div className="boot"><div className="boot-orbit" /><span>initializing local-router control plane</span></div>;

  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand-lockup"><div className="brand-mark"><span /><span /><span /></div><div><div className="brand-name">local-router</div><div className="brand-sub">control plane / {data.status.version}</div></div></div>
      <div className="rail-label">Workspace</div>
      <nav className="side-nav" aria-label="Primary navigation">
        {tabs.map((item) => <button key={item.id} className={tab === item.id ? "nav-item nav-active" : "nav-item"} onClick={() => setTab(item.id)}><Icon name={item.icon} /><span>{item.label}</span>{tab === item.id && <i />}</button>)}
      </nav>
      <div className="sidebar-foot"><div className="connection"><StatusDot /><div><strong>Gateway online</strong><span>{data.status.settings.gateway_addr}:{data.status.settings.gateway_port}</span></div></div><div className="sidebar-meta">LOCAL ONLY / API KEYS STAY HERE</div></div>
    </aside>
    <main className="main-area">
      <header className="topbar"><div><div className="breadcrumb">LOCAL-ROUTER <span>/</span> {currentTab.label.toUpperCase()}</div><h1>{currentTab.label}</h1></div><div className="top-actions"><div className="runtime-chip"><StatusDot /> <span>runtime ready</span></div><Button icon="refresh" variant="secondary" onClick={() => reload().then(() => notify("Workspace refreshed")).catch((error: Error) => notify(error.message, true))}>Refresh</Button></div></header>
      {tab === "overview" && <Overview data={data} usage={usage} workbuddy={workbuddy} onOpen={(next) => setTab(next)} />}
      {tab === "providers" && <Providers providers={data.providers} onChanged={reload} notify={notify} />}
      {tab === "models" && <Models models={data.models} />}
      {tab === "groups" && <Groups groups={data.groups} models={data.models} onChanged={reload} notify={notify} />}
      {tab === "usage" && <Usage notify={notify} />}
      {tab === "settings" && <SettingsPanel settings={data.status.settings} callerKeys={callerKeys} setCallerKeys={setCallerKeys} onChanged={reload} notify={notify} />}
    </main>
    {notice && <div className={`notice ${notice.error ? "notice-error" : ""}`}>{notice.text}</div>}
  </div>;
}

function Overview({ data, usage, workbuddy, onOpen }: { data: AppData; usage: UsageData | null; workbuddy: WorkBuddyStatus | null; onOpen: (tab: Tab) => void }) {
  const settings = data.status.settings;
  const endpoint = `http://${settings.gateway_addr}:${settings.gateway_port}`;
  const recent = usage?.recent ?? [];
  return <div className="page-grid">
    <div className="signal-row"><div className="signal-card signal-wide"><span className="signal-label">SYSTEM STATUS</span><div className="signal-main"><StatusDot /><strong>Gateway operational</strong><span>All request paths are accepting traffic</span></div><div className="signal-path">{endpoint}</div></div><div className="signal-card"><span className="signal-label">PROVIDERS</span><strong className="metric-value">{data.providers.length.toString().padStart(2, "0")}</strong><span className="metric-detail">configured upstreams</span></div><div className="signal-card"><span className="signal-label">MODELS</span><strong className="metric-value">{data.models.length.toString().padStart(2, "0")}</strong><span className="metric-detail">available endpoints</span></div></div>
    <div className="content-grid content-grid-main"><Panel eyebrow="01 / quick access" title="Operator surface" action={<span className="panel-code">LOCAL / {data.status.version}</span>}><div className="operator-grid"><button onClick={() => onOpen("providers")}><span className="operator-icon"><Icon name="server" /></span><span><strong>Manage providers</strong><small>Keys, endpoints, model catalog</small></span><span className="arrow">↗</span></button><button onClick={() => onOpen("groups")}><span className="operator-icon"><Icon name="activity" /></span><span><strong>Configure routing</strong><small>Failover, rotation, affinity</small></span><span className="arrow">↗</span></button><button onClick={() => onOpen("models")}><span className="operator-icon"><Icon name="layers" /></span><span><strong>Browse model map</strong><small>Every exposed model in one view</small></span><span className="arrow">↗</span></button><button onClick={() => onOpen("usage")}><span className="operator-icon"><Icon name="grid" /></span><span><strong>Inspect usage</strong><small>Tokens, latency, request history</small></span><span className="arrow">↗</span></button></div></Panel><Panel eyebrow="02 / connection" title="Agent environment"><div className="code-block"><div><span className="code-key">OPENAI_BASE_URL</span><span>=</span><span>{endpoint}/v1</span></div><div><span className="code-key">ANTHROPIC_BASE_URL</span><span>=</span><span>{endpoint}</span></div><div><span className="code-key">LOCAL_ROUTER_API_KEY</span><span>=</span><span>{settings.default_caller_key}</span></div></div><div className="panel-footnote"><Icon name="terminal" /> Export these variables to connect any compatible agent.</div></Panel></div>
    <div className="content-grid content-grid-secondary"><Panel eyebrow="03 / live feed" title="Recent requests" action={<button className="text-action" onClick={() => onOpen("usage")}>View all ↗</button>}>{recent.length ? <div className="request-list">{recent.slice(0, 5).map((record, index) => <RequestRow key={`${record.time}-${index}`} record={record} />)}</div> : <EmptyState title="No requests yet" detail="Traffic will appear here when an agent connects." />}</Panel><Panel eyebrow="04 / routing posture" title="Groups at a glance"><div className="group-list">{data.groups.length ? data.groups.slice(0, 4).map((group) => <div className="mini-group" key={group.id}><div><strong>{group.name}</strong><span>{group.models.length} members / {group.routing}</span></div><span className="group-id">group/{group.id}</span></div>) : <EmptyState title="No routing groups" detail="Create a group to add failover or rotation." />}</div></Panel></div>
    <Panel eyebrow="05 / account" title="WorkBuddy session"><WorkBuddyPanel status={workbuddy} /></Panel>
  </div>;
}

function WorkBuddyPanel({ status }: { status: WorkBuddyStatus | null }) {
  if (!status) return <EmptyState title="Reading the session" detail="Looking for the local WorkBuddy login." />;
  if (!status.signed_in) return <EmptyState title="Not signed in to WorkBuddy" detail={status.note ?? "Open WorkBuddy and sign in; local-router then reuses that session."} />;
  const credits = status.credits;
  const checkin = credits?.checkin;
  return <div className="workbuddy-panel">
    <div className="signal-row">
      <div className="signal-card"><span className="signal-label">ACCOUNT</span><strong className="metric-value">{status.account?.nickname || "signed in"}</strong><span className="metric-detail">{status.account?.type || "personal"}</span></div>
      <div className="signal-card"><span className="signal-label">CREDITS LEFT</span><strong className="metric-value">{(credits?.remaining ?? 0).toString()}</strong><span className="metric-detail">of {(credits?.capacity ?? 0).toString()} this cycle</span></div>
      <div className="signal-card"><span className="signal-label">CHECK-IN</span><strong className="metric-value">{checkin ? (checkin.checked_in_today ? "DONE" : "TODO") : "--"}</strong><span className="metric-detail">{checkin ? `streak ${checkin.streak_days} days · ${checkin.today_credit} credits` : "unknown"}</span></div>
      <div className="signal-card"><span className="signal-label">TOKEN</span><strong className="metric-value">{status.token_expires_at ? formatTime(new Date(status.token_expires_at).toISOString()) : "--"}</strong><span className="metric-detail">access token expiry</span></div>
    </div>
    {credits?.packages.length ? <div className="model-table"><div className="model-head"><span>PACKAGE</span><span>REMAINING</span><span>CAPACITY</span><span>USED</span></div>{credits.packages.map((item, index) => <div className="model-row" key={`${item.package_name}-${index}`}><div className="model-name"><span className="model-bullet" /> <strong>{item.package_name}</strong></div><span>{item.remaining}</span><span>{item.capacity} {item.unit}</span><span>{item.used}</span></div>)}</div> : null}
  </div>;
}

/// 按天柱状图：请求数 + 每天的美元成本。
function DayChart({ data }: { data: [string, UsageTotals][] }) {
  if (!data.length) return <EmptyState title="No daily activity" detail="The activity graph will populate automatically." />;
  const days = data.slice(-14);
  const maxReq = Math.max(...days.map(([, v]) => v.requests), 1);
  const maxCost = Math.max(...days.map(([, v]) => v.cost_usd), 0.000001);
  return <div className="chart-block"><div className="bar-chart">{days.map(([day, v]) => <div className="bar-col" key={day} title={`${day}: ${v.requests} requests · $${v.cost_usd.toFixed(4)}`}><div className="bar-stack"><i className="bar-cost" style={{ height: `${(v.cost_usd / maxCost) * 100}%` }} /><i className="bar-req" style={{ height: `${(v.requests / maxReq) * 100}%` }} /></div><span>{day.slice(5)}</span></div>)}</div><div className="chart-legend"><span><i className="dot dot-req" />requests</span><span><i className="dot dot-cost" />cost (scaled)</span></div></div>;
}

/// 每平台成本横条：谁是花钱大头一眼可见。
function ProviderCostChart({ data }: { data: [string, UsageTotals][] }) {
  if (!data.length) return <EmptyState title="No platform usage" detail="Platform costs appear after the first call." />;
  const maxCost = Math.max(...data.map(([, v]) => v.cost_usd), 0.000001);
  const maxCredits = Math.max(...data.map(([, v]) => v.credits), 0.000001);
  return <div className="hbar-chart">{data.map(([name, v]) => <div className="hbar-row" key={name}><div className="hbar-head"><strong>{name}</strong><span>{v.requests} req · ${v.cost_usd.toFixed(4)} · {v.credits.toFixed(1)} credits</span></div><div className="hbar-track"><i className="hbar-usd" style={{ width: `${(v.cost_usd / maxCost) * 100}%` }} /><i className="hbar-credits" style={{ width: `${(v.credits / maxCredits) * 100}%` }} /></div></div>)}</div>;
}

/// 模型占比甜甜圈：SVG 手绘，无第三方依赖。
function ModelDonut({ data }: { data: [string, UsageTotals][] }) {
  const models = data.filter(([, v]) => v.requests > 0).slice(0, 8);
  if (!models.length) return <EmptyState title="No model usage" detail="The mix appears after the first call." />;
  const total = models.reduce((sum, [, v]) => sum + v.requests, 0);
  const colors = ["#9ef0bf", "#5ec9a8", "#3e9e7e", "#7ec9e8", "#a89ef0", "#f0d29e", "#f09e9e", "#9ef0e0"];
  const radius = 52, stroke = 16, circumference = 2 * Math.PI * radius;
  let offset = 0;
  return <div className="donut-block"><svg viewBox="0 0 140 140" className="donut">{models.map(([name, v], i) => { const frac = v.requests / total; const dash = frac * circumference; const el = <circle key={name} cx="70" cy="70" r={radius} fill="none" stroke={colors[i % colors.length]} strokeWidth={stroke} strokeDasharray={`${dash} ${circumference - dash}`} strokeDashoffset={-offset} transform="rotate(-90 70 70)" />; offset += dash; return el; })}</svg><div className="donut-legend">{models.map(([name, v], i) => <div className="legend-row" key={name}><i style={{ background: colors[i % colors.length] }} /><span title={name}>{name}</span><strong>{Math.round((v.requests / total) * 100)}%</strong></div>)}</div></div>;
}

function RequestRow({ record }: { record: UsageData["recent"][number] }) {
  return <div className="request-row"><span className={`request-status ${record.ok ? "ok" : "fail"}`} /><div className="request-model"><strong>{record.model}</strong><span>{record.provider} / {record.protocol}</span></div><span className="request-time">{formatTime(record.time)}</span><span className="request-latency">{record.latency_ms}ms</span><span className={`request-result ${record.ok ? "" : "result-fail"}`}>{record.ok ? "OK" : "FAIL"}</span></div>;
}

function Providers({ providers, onChanged, notify }: { providers: Provider[]; onChanged: () => Promise<void>; notify: (text: string, error?: boolean) => void }) {
  const [showAdd, setShowAdd] = useState(false);
  const [presets, setPresets] = useState<{ group: string; presets: Preset[] }[]>([]);
  const [selected, setSelected] = useState<Preset | null>(null);
  const [form, setForm] = useState({ name: "", url: "", key: "", models: "" });
  const openAdd = async () => { const response = await getPresets(); setPresets(response.presets); setShowAdd(true); };
  const choose = (preset: Preset) => { setSelected(preset); setForm({ name: preset.name, url: preset.base_url ?? "", key: "", models: preset.default_models.join(", ") }); };
  const create = async () => { try { await api("/api/providers", { method: "POST", body: { preset: selected?.id ?? "custom", name: form.name, base_url: form.url || null, key: form.key || null, models: form.models.split(",").map((item) => item.trim()).filter(Boolean) } }); notify("Provider added"); setShowAdd(false); setSelected(null); await onChanged(); } catch (error) { notify((error as Error).message, true); } };
  const toggle = async (provider: Provider) => { try { await api(`/api/providers/${provider.id}`, { method: "PATCH", body: { enabled: !provider.enabled } }); notify(`${provider.name} ${provider.enabled ? "paused" : "enabled"}`); await onChanged(); } catch (error) { notify((error as Error).message, true); } };
  const remove = async (provider: Provider) => { if (!window.confirm(`Remove ${provider.name}?`)) return; try { await api(`/api/providers/${provider.id}`, { method: "DELETE" }); notify("Provider removed"); await onChanged(); } catch (error) { notify((error as Error).message, true); } };
  return <div className="page-grid"><div className="section-toolbar"><div><span className="eyebrow">REGISTRY / UPSTREAMS</span><p>Each provider is an isolated route into your model network.</p></div><Button variant="primary" icon="plus" onClick={openAdd}>Add provider</Button></div><div className="provider-grid">{providers.map((provider) => <article className="provider-card" key={provider.id}><div className="provider-top"><div className="provider-icon"><Icon name="server" /></div><div className="provider-title"><strong>{provider.name}</strong><span>{provider.id} / {provider.protocol}</span></div><StatusDot active={provider.enabled} /></div><div className="provider-url">{provider.base_url ?? provider.anthropic_base_url ?? "No endpoint"}</div><div className="provider-meta"><span>{provider.auth ? "登录态" : `${provider.keys.length} key${provider.keys.length === 1 ? "" : "s"}`}</span><span>{provider.models.length} models</span>{provider.media_models?.length ? <span>{provider.media_models.length} media</span> : null}<span className={provider.enabled ? "state-on" : "state-off"}>{provider.enabled ? "ACTIVE" : "PAUSED"}</span></div><div className="provider-models">{provider.models.slice(0, 4).map((model) => <span key={model}>{model}</span>)}{provider.models.length > 4 && <span>+{provider.models.length - 4} more</span>}{provider.media_models?.map((model) => <span key={`media-${model}`} className="model-media">{model}</span>)}</div><div className="card-actions"><Button variant="quiet" onClick={() => toggle(provider)}>{provider.enabled ? "Pause" : "Enable"}</Button><Button variant="danger" icon="trash" onClick={() => remove(provider)}>Remove</Button></div></article>)}</div>{showAdd && <Modal title="Add provider" onClose={() => setShowAdd(false)}><div className="modal-layout"><div className="preset-list">{presets.map((group) => <div key={group.group}><span className="preset-group">{group.group}</span>{group.presets.map((preset) => <button className={selected?.id === preset.id ? "preset-option preset-selected" : "preset-option"} onClick={() => choose(preset)} key={preset.id}><strong>{preset.name}</strong><small>{preset.protocol} · {preset.key_hint}</small></button>)}</div>)}</div><div className="form-stack"><Field label="Name"><input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} /></Field><Field label="Base URL"><input value={form.url} onChange={(event) => setForm({ ...form, url: event.target.value })} /></Field><Field label="API key"><input type="password" value={form.key} onChange={(event) => setForm({ ...form, key: event.target.value })} /></Field><Field label="Exposed models"><input value={form.models} onChange={(event) => setForm({ ...form, models: event.target.value })} /><small className="field-help">Comma-separated model IDs. Refresh the catalog later from the CLI.</small></Field></div></div><div className="modal-footer"><Button variant="secondary" onClick={() => setShowAdd(false)}>Cancel</Button><Button variant="primary" onClick={create}>Add provider</Button></div></Modal>}</div>;
}

function Models({ models }: { models: ModelItem[] }) {
  const [query, setQuery] = useState("");
  const filtered = useMemo(() => models.filter((model) => model.id.toLowerCase().includes(query.toLowerCase())), [models, query]);
  return <div className="page-grid"><div className="section-toolbar"><div><span className="eyebrow">CATALOG / EXPOSED MODELS</span><p>A flat, addressable map for every agent and protocol.</p></div><div className="search-box"><Icon name="search" /><input placeholder="Filter models" value={query} onChange={(event) => setQuery(event.target.value)} /></div></div><Panel title={`${filtered.length.toString().padStart(2, "0")} exposed models`} action={<span className="panel-code">provider/model</span>}>{filtered.length ? <div className="model-table"><div className="model-head"><span>MODEL ID</span><span>OWNER</span><span>STATE</span><span>TYPE</span></div>{filtered.map((model) => <div className="model-row" key={model.id}><div className="model-name"><span className="model-bullet" /> <strong>{model.id}</strong></div><span>{model.provider_name}</span><span className={model.enabled ? "state-on" : "state-off"}>{model.enabled ? "AVAILABLE" : "PAUSED"}</span><span className="mono-tag">{model.group ? "ROUTING GROUP" : "UPSTREAM"}</span></div>)}</div> : <EmptyState title="No matching models" detail="Try another filter or expose a model from a provider." />}</Panel></div>;
}

function Groups({ groups, models, onChanged, notify }: { groups: Group[]; models: ModelItem[]; onChanged: () => Promise<void>; notify: (text: string, error?: boolean) => void }) {
  const [showAdd, setShowAdd] = useState(false);
  const [form, setForm] = useState({ name: "", models: "", routing: "order", stays: "auto" });
  const create = async () => { try { await api("/api/groups", { method: "POST", body: { name: form.name, models: form.models.split(",").map((item) => item.trim()).filter(Boolean), routing: form.routing, stays: form.stays, listed: true } }); notify("Routing group added"); setShowAdd(false); setForm({ name: "", models: "", routing: "order", stays: "auto" }); await onChanged(); } catch (error) { notify((error as Error).message, true); } };
  const remove = async (group: Group) => { if (!window.confirm(`Remove ${group.name}?`)) return; try { await api(`/api/groups/${group.id}`, { method: "DELETE" }); notify("Routing group removed"); await onChanged(); } catch (error) { notify((error as Error).message, true); } };
  return <div className="page-grid"><div className="section-toolbar"><div><span className="eyebrow">ROUTER / POLICY SETS</span><p>Turn several upstream models into one resilient endpoint.</p></div><Button variant="primary" icon="plus" onClick={() => setShowAdd(true)}>Add routing group</Button></div><div className="routing-grid">{groups.map((group) => <article className="routing-card" key={group.id}><div className="routing-header"><div><span className="routing-code">group/{group.id}</span><h3>{group.name}</h3></div><span className="route-mode">{group.routing}</span></div><div className="route-line"><span>STAYS</span><strong>{group.stays}</strong><span>MEMBERS</span><strong>{group.models.length.toString().padStart(2, "0")}</strong></div><div className="route-members">{group.models.map((model, index) => <div key={model}><i>{String(index + 1).padStart(2, "0")}</i><span>{model}</span>{index === 0 && group.routing === "order" && <em>PRIMARY</em>}</div>)}</div><div className="card-actions"><span className="route-health"><StatusDot /> route ready</span><Button variant="danger" icon="trash" onClick={() => remove(group)}>Remove</Button></div></article>)}</div>{showAdd && <Modal title="Create routing group" onClose={() => setShowAdd(false)}><div className="form-stack"><Field label="Group name"><input autoFocus value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} placeholder="Daily fallback" /></Field><Field label="Members"><input value={form.models} onChange={(event) => setForm({ ...form, models: event.target.value })} placeholder="provider/model, provider/model" /><small className="field-help">Available now: {models.slice(0, 3).map((model) => model.id).join(" · ")}</small></Field><Field label="Routing"><select value={form.routing} onChange={(event) => setForm({ ...form, routing: event.target.value })}><option value="order">order — primary then failover</option><option value="rotate">rotate — round robin</option><option value="usage">usage — least used</option><option value="smart">smart — quota-aware</option><option value="weight">weight — weighted interleave</option></select></Field><Field label="Session affinity"><select value={form.stays} onChange={(event) => setForm({ ...form, stays: event.target.value })}><option value="auto">auto — preserve useful cache</option><option value="session">session — keep the session</option><option value="turn">turn — keep the turn</option><option value="off">off — no affinity</option><option value="strict">strict — never fail over</option></select></Field></div><div className="modal-footer"><Button variant="secondary" onClick={() => setShowAdd(false)}>Cancel</Button><Button variant="primary" onClick={create}>Create group</Button></div></Modal>}</div>;
}

type UsageView = "overview" | "sessions" | "projects" | "context" | "quota" | "bills";

const usageViews: { id: UsageView; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "sessions", label: "Sessions" },
  { id: "projects", label: "Projects" },
  { id: "context", label: "Context" },
  { id: "quota", label: "Quota" },
  { id: "bills", label: "Bills" },
];

const usagePeriods: { id: string; label: string }[] = [
  { id: "today", label: "Today" },
  { id: "7d", label: "7 days" },
  { id: "30d", label: "30 days" },
  { id: "all", label: "All" },
];

function Usage({ notify }: { notify: (text: string, error?: boolean) => void }) {
  const [view, setView] = useState<UsageView>("overview");
  const [period, setPeriod] = useState("7d");
  const [usage, setUsage] = useState<UsageData | null>(null);
  const [projects, setProjects] = useState<ProjectRow[]>([]);

  const reload = useCallback(async () => {
    const [data, projectData] = await Promise.all([getUsage(period), getProjects(period)]);
    setUsage(data);
    setProjects(projectData.projects);
  }, [period]);

  useEffect(() => {
    reload().catch((error: Error) => notify(error.message, true));
  }, [reload, notify]);

  return <div className="page-grid">
    <div className="usage-toolbar">
      <div className="subtabs" role="tablist">
        {usageViews.map((item) => <button key={item.id} role="tab" aria-selected={view === item.id} className={view === item.id ? "subtab subtab-active" : "subtab"} onClick={() => setView(item.id)}>{item.label}</button>)}
      </div>
      <div className="period-picker">
        {usagePeriods.map((item) => <button key={item.id} className={period === item.id ? "period period-active" : "period"} onClick={() => setPeriod(item.id)}>{item.label}</button>)}
      </div>
    </div>
    {!usage ? <div className="skeleton-page"><div /><div /><div /></div> : <>
      {view === "overview" && <UsageOverview usage={usage} notify={notify} />}
      {view === "sessions" && <SessionsPanel sessions={usage.by_session} />}
      {view === "projects" && <ProjectsPanel projects={projects} onChanged={reload} notify={notify} />}
      {view === "context" && <ContextPanel usage={usage} />}
      {view === "quota" && <QuotaPanel usage={usage} />}
      {view === "bills" && <BillsPanel usage={usage} />}
    </>}
  </div>;
}

/// The overview: totals, the requests timeline, and the per-platform / model
/// breakdowns the page has always had.
function UsageOverview({ usage, notify }: { usage: UsageData; notify: (text: string, error?: boolean) => void }) {
  const totals = usage.totals;
  const perProvider = Object.entries(usage.by_provider_costs ?? {});
  const cacheRate = totals.input_tokens + totals.cache_read_tokens > 0
    ? totals.cache_read_tokens / (totals.input_tokens + totals.cache_read_tokens)
    : 0;
  return <>
    <span className="storage-tag">{usage.storage === "sqlite" ? "SQLITE" : "JSONL"}</span>
    <div className="metric-strip"><Metric label="REQUESTS" value={totals.requests.toLocaleString()} /><Metric label="FAILED" value={totals.failed.toLocaleString()} danger={totals.failed > 0} /><Metric label="INPUT TOKENS" value={formatTokens(totals.input_tokens)} /><Metric label="OUTPUT TOKENS" value={formatTokens(totals.output_tokens)} /><Metric label="CACHE HIT" value={`${Math.round(cacheRate * 100)}%`} /><Metric label="COST (USD)" value={`$${totals.cost_usd.toFixed(4)}`} /><Metric label="CREDITS" value={totals.credits.toFixed(1)} /></div>
    <Panel eyebrow="USAGE / REQUESTS TIMELINE" title="Requests over time" action={<TimelineToggle />}>
      <RequestsTimeline points={usage.requests_timeline} />
    </Panel>
    <div className="content-grid content-grid-secondary">
      <Panel eyebrow="USAGE / BY PLATFORM" title="What each provider cost">
        <div className="usage-list">{perProvider.length ? perProvider.map(([name, value]) => <div className="usage-row" key={name}><div><strong>{name}</strong><span>{value.requests} requests · {formatTokens(value.input_tokens)} in · {formatTokens(value.output_tokens)} out</span></div><div className="usage-money"><span className="usage-number">${value.cost_usd.toFixed(4)}</span><span className="usage-credits">{value.credits.toFixed(1)} credits{value.unpriced > 0 ? ` · ${value.unpriced} unpriced` : ""}</span></div></div>) : <EmptyState title="No platform usage" detail="Platform costs appear after the first call." />}</div>
      </Panel>
      <Panel eyebrow="USAGE / BY DAY" title="Daily activity"><DayChart data={usage.by_day} /></Panel>
    </div>
    <div className="content-grid content-grid-secondary">
      <Panel eyebrow="USAGE / BY MODEL" title="Model distribution">
        <div className="usage-list">{usage.by_model.length ? usage.by_model.map(([name, value]) => <div className="usage-row" key={name}><div><strong>{name}</strong><span>{value.requests} requests · {formatTokens(value.output_tokens)} output</span></div><span className="usage-number">{value.requests}</span></div>) : <EmptyState title="No usage data" detail="Request data appears after the first call." />}</div>
      </Panel>
      <Panel eyebrow="USAGE / SHARE" title="Model mix"><ModelDonut data={usage.by_model} /></Panel>
    </div>
    <Panel eyebrow="USAGE / REQUEST LOG" title="Recent requests" action={<Button variant="quiet" onClick={() => getUsage().then(() => notify("Usage refreshed"))}>Refresh log</Button>}>{usage.recent.length ? <div className="request-list">{usage.recent.map((record, index) => <RequestRow key={`${record.time}-${index}`} record={record} />)}</div> : <EmptyState title="No requests yet" detail="Connect an agent to start collecting telemetry." />}</Panel>
  </>;
}

/// Requests per hour: bars by default, a line when the viewer prefers it.
/// The preference is per browser, the way magpie keeps it.
let timelineStyle: "bars" | "line" = (localStorage.getItem("local-router.timeline") as "bars" | "line") || "bars";

function TimelineToggle() {
  const [style, setStyle] = useState(timelineStyle);
  const pick = (next: "bars" | "line") => {
    timelineStyle = next;
    localStorage.setItem("local-router.timeline", next);
    window.dispatchEvent(new CustomEvent("local-router.timeline", { detail: next }));
    setStyle(next);
  };
  return <span className="timeline-toggle">
    <button className={style === "bars" ? "period period-active" : "period"} onClick={() => pick("bars")}>Bars</button>
    <button className={style === "line" ? "period period-active" : "period"} onClick={() => pick("line")}>Line</button>
  </span>;
}

function RequestsTimeline({ points }: { points: TimelinePoint[] }) {
  const [style, setStyle] = useState(timelineStyle);
  useEffect(() => {
    const listen = (event: Event) => setStyle((event as CustomEvent).detail as "bars" | "line");
    window.addEventListener("local-router.timeline", listen);
    return () => window.removeEventListener("local-router.timeline", listen);
  }, []);
  if (!points.length) return <EmptyState title="No requests yet" detail="The timeline fills as agents call the gateway." />;
  const max = Math.max(...points.map((p) => p.requests), 1);
  const width = Math.max(points.length * 34, 320);
  const height = 150;
  if (style === "line") {
    const step = points.length > 1 ? (width - 30) / (points.length - 1) : 0;
    const path = points.map((p, i) => `${i === 0 ? "M" : "L"} ${15 + i * step} ${height - 24 - (p.requests / max) * (height - 44)}`).join(" ");
    return <div className="timeline-block"><svg viewBox={`0 0 ${width} ${height}`} className="timeline-svg"><path d={path} fill="none" stroke="var(--green)" strokeWidth="1.6" strokeLinejoin="round" />{points.map((p, i) => <circle key={p.hour} cx={15 + i * step} cy={height - 24 - (p.requests / max) * (height - 44)} r="2.4" fill="var(--green)" />)}{points.map((p, i) => (i % Math.ceil(points.length / 8) === 0 ? <text key={`t-${p.hour}`} x={15 + i * step} y={height - 6} textAnchor="middle" className="timeline-tick">{p.hour.slice(5, 13)}</text> : null))}</svg></div>;
  }
  return <div className="timeline-block"><div className="timeline-bars" style={{ minWidth: width }}>{points.map((p) => <div className="timeline-col" key={p.hour} title={`${p.hour}:00 — ${p.requests} requests`}><i style={{ height: `${(p.requests / max) * 100}%` }} /><span>{p.hour.slice(5, 13)}</span></div>)}</div></div>;
}

/// Every conversation the gateway has seen, titled and priced.
function SessionsPanel({ sessions }: { sessions: SessionRow[] }) {
  const [query, setQuery] = useState("");
  const filtered = sessions.filter((s) => s.title.toLowerCase().includes(query.toLowerCase()) || s.id.toLowerCase().includes(query.toLowerCase()));
  return <Panel eyebrow="USAGE / SESSIONS" title={`${sessions.length.toString().padStart(2, "0")} conversations`} action={<div className="search-box"><Icon name="search" /><input placeholder="Filter sessions" value={query} onChange={(event) => setQuery(event.target.value)} /></div>}>
    {filtered.length ? <div className="session-table">
      <div className="model-head session-head"><span>SESSION</span><span>REQUESTS</span><span>TOKENS</span><span>COST</span><span>LAST SEEN</span></div>
      {filtered.map((s) => <div className="model-row session-row" key={s.id} title={s.id}>
        <div className="model-name"><span className="model-bullet" /><strong>{s.title}</strong></div>
        <span>{s.requests}{s.failed ? ` · ${s.failed} failed` : ""}</span>
        <span>{formatTokens(s.input_tokens)} in / {formatTokens(s.output_tokens)} out</span>
        <span className="usage-number">${s.cost_usd.toFixed(4)}{s.credits > 0 ? ` · ${s.credits.toFixed(1)} cr` : ""}</span>
        <span>{s.last_seen ? formatTime(s.last_seen) : "—"}</span>
      </div>)}
    </div> : <EmptyState title="No sessions yet" detail="Sessions appear when an agent sends a session id (metadata.user_id or x-session-id)." />}
  </Panel>;
}

/// Projects: usage grouped by the working directory agents run in. Rename,
/// color, budget, and merge — the management magpie offers per folder.
function ProjectsPanel({ projects, onChanged, notify }: { projects: ProjectRow[]; onChanged: () => Promise<void>; notify: (text: string, error?: boolean) => void }) {
  const [editing, setEditing] = useState<ProjectRow | null>(null);
  const [mergeFrom, setMergeFrom] = useState<string | null>(null);
  const save = async (id: string, patch: { name?: string; color?: string; budget_tokens?: number | null; budget_cost?: number | null }) => {
    try {
      await updateProject(id, patch);
      setEditing(null);
      await onChanged();
      notify("Project updated");
    } catch (error) {
      notify((error as Error).message, true);
    }
  };
  const merge = async (from: string, into: string) => {
    if (!from || !into || from === into) return;
    if (!window.confirm(`Merge all usage of\n${from}\ninto\n${into}? This re-keys the history.`)) return;
    try {
      await mergeProjects(from, into);
      setMergeFrom(null);
      notify("Projects merged");
      await onChanged();
    } catch (error) {
      notify((error as Error).message, true);
    }
  };
  return <Panel eyebrow="USAGE / PROJECTS" title={`${projects.length.toString().padStart(2, "0")} projects`} action={<Button variant="quiet" icon="refresh" onClick={() => onChanged().then(() => notify("Projects refreshed"))}>Refresh</Button>}>
    {projects.length ? <div className="project-list">
      {projects.map((project) => {
        const tokenBudget = project.budget_tokens ? Math.min(1, project.input_tokens / project.budget_tokens) : null;
        const costBudget = project.budget_cost ? Math.min(1, project.cost_usd / project.budget_cost) : null;
        return <div className="project-card" key={project.id}>
          <div className="project-top">
            <span className="project-dot" style={{ background: project.color }} />
            <div className="project-title">
              <strong>{project.name}</strong>
              <span title={project.id}>{project.id}</span>
            </div>
            <div className="project-actions">
              <Button variant="quiet" onClick={() => setEditing(project)}>Edit</Button>
              {mergeFrom === null
                ? <Button variant="quiet" onClick={() => setMergeFrom(project.id)}>Merge…</Button>
                : <select className="merge-select" defaultValue="" onChange={(event) => { const into = event.target.value; setMergeFrom(null); merge(project.id, into); }}>
                    <option value="" disabled>Merge into…</option>
                    {projects.filter((p) => p.id !== project.id).map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
                  </select>}
            </div>
          </div>
          <div className="project-stats">
            <span>{project.requests} requests{project.failed ? ` · ${project.failed} failed` : ""}</span>
            <span>{formatTokens(project.input_tokens)} in · {formatTokens(project.output_tokens)} out</span>
            <span className="usage-number">${project.cost_usd.toFixed(4)}</span>
            {project.credits > 0 && <span className="usage-credits">{project.credits.toFixed(1)} credits</span>}
            <span className="project-last">{project.last_seen ? `last ${formatTime(project.last_seen)}` : "no requests in range"}</span>
          </div>
          {(tokenBudget !== null || costBudget !== null) && <div className="project-budgets">
            {tokenBudget !== null && <div className="budget-row"><span>TOKENS</span><div className="budget-track"><i className={tokenBudget > 0.9 ? "budget-fill budget-hot" : "budget-fill"} style={{ width: `${tokenBudget * 100}%` }} /></div><span>{Math.round(tokenBudget * 100)}%</span></div>}
            {costBudget !== null && <div className="budget-row"><span>COST</span><div className="budget-track"><i className={costBudget > 0.9 ? "budget-fill budget-hot" : "budget-fill"} style={{ width: `${costBudget * 100}%` }} /></div><span>{Math.round(costBudget * 100)}%</span></div>}
          </div>}
        </div>;
      })}
    </div> : <EmptyState title="No projects yet" detail="Projects appear when an agent reports its working directory (x-project / x-cwd header, metadata.cwd, or Claude Code's metadata.user_id)." />}
    {editing && <ProjectEditModal project={editing} onClose={() => setEditing(null)} onSave={save} />}
  </Panel>;
}

function ProjectEditModal({ project, onClose, onSave }: { project: ProjectRow; onClose: () => void; onSave: (id: string, patch: { name?: string; color?: string; budget_tokens?: number | null; budget_cost?: number | null }) => void }) {
  const [name, setName] = useState(project.name);
  const [color, setColor] = useState(project.color);
  const [budgetTokens, setBudgetTokens] = useState(project.budget_tokens?.toString() ?? "");
  const [budgetCost, setBudgetCost] = useState(project.budget_cost?.toString() ?? "");
  return <Modal title="Edit project" onClose={onClose}>
    <div className="form-stack">
      <Field label="Name"><input autoFocus value={name} onChange={(event) => setName(event.target.value)} placeholder={project.id} /></Field>
      <Field label="Color"><input value={color} onChange={(event) => setColor(event.target.value)} placeholder="#9ef0bf" /><small className="field-help">Any CSS color.</small></Field>
      <Field label="Token budget"><input type="number" value={budgetTokens} onChange={(event) => setBudgetTokens(event.target.value)} placeholder="no budget" /><small className="field-help">A cap for the bar; leave empty for none.</small></Field>
      <Field label="Cost budget (USD)"><input type="number" value={budgetCost} onChange={(event) => setBudgetCost(event.target.value)} placeholder="no budget" /></Field>
    </div>
    <div className="modal-footer">
      <Button variant="secondary" onClick={onClose}>Cancel</Button>
      <Button variant="primary" onClick={() => onSave(project.id, {
        name,
        color,
        budget_tokens: budgetTokens.trim() ? Number(budgetTokens) : null,
        budget_cost: budgetCost.trim() ? Number(budgetCost) : null,
      })}>Save</Button>
    </div>
  </Modal>;
}

/// How full each request's window was, and what filled it.
function ContextPanel({ usage }: { usage: UsageData }) {
  const rows = usage.context.by_provider;
  const t = usage.context.totals;
  const total = t.system_tokens + t.tool_tokens + t.file_tokens + t.conversation_tokens || 1;
  return <>
    <Panel eyebrow="USAGE / CONTEXT" title="What fills the window">
      {!usage.context.captured ? <EmptyState title="No context captured yet" detail="The breakdown appears after the next request through the gateway." /> : <div className="context-totals">
        <div className="context-bar">
          <i className="ctx-system" style={{ width: `${(t.system_tokens / total) * 100}%` }} title={`System: ${formatTokens(t.system_tokens)}`} />
          <i className="ctx-tools" style={{ width: `${(t.tool_tokens / total) * 100}%` }} title={`Tools: ${formatTokens(t.tool_tokens)}`} />
          <i className="ctx-files" style={{ width: `${(t.file_tokens / total) * 100}%` }} title={`Files: ${formatTokens(t.file_tokens)}`} />
          <i className="ctx-conv" style={{ width: `${(t.conversation_tokens / total) * 100}%` }} title={`Conversation: ${formatTokens(t.conversation_tokens)}`} />
        </div>
        <div className="context-legend">
          <span><i className="dot dot-system" />system {formatTokens(t.system_tokens)}</span>
          <span><i className="dot dot-tools" />tools {formatTokens(t.tool_tokens)}</span>
          <span><i className="dot dot-files" />files {formatTokens(t.file_tokens)}</span>
          <span><i className="dot dot-conv" />conversation {formatTokens(t.conversation_tokens)}</span>
        </div>
      </div>}
    </Panel>
    <Panel eyebrow="USAGE / CONTEXT BY PROVIDER" title="Per platform">
      {rows.length ? <div className="model-table">
        <div className="model-head"><span>PROVIDER</span><span>REQUESTS</span><span>AVG FILL</span><span>SYSTEM / TOOLS / FILES / CONV</span></div>
        {rows.map((row) => <div className="model-row" key={row.provider}>
          <div className="model-name"><span className="model-bullet" /><strong>{row.provider}</strong></div>
          <span>{row.requests}</span>
          <span>{row.avg_fill > 0 ? `${Math.round(row.avg_fill * 100)}%` : "—"}</span>
          <span>{formatTokens(row.system_tokens)} / {formatTokens(row.tool_tokens)} / {formatTokens(row.file_tokens)} / {formatTokens(row.conversation_tokens)}</span>
        </div>)}
      </div> : <EmptyState title="No provider breakdown" detail="Fill levels appear after the next request." />}
    </Panel>
  </>;
}

/// Balances and quota windows per account, the way magpie lists plans.
function QuotaPanel({ usage }: { usage: UsageData }) {
  const accounts = usage.quota ?? [];
  return <Panel eyebrow="USAGE / QUOTA" title="Balances and windows">
    {accounts.length ? <div className="quota-list">
      {accounts.map((account) => <div className="quota-account" key={account.provider}>
        <div className="quota-head"><strong>{account.name}</strong><span>{account.kind}{account.user ? ` · ${account.user}` : ""}{account.unlimited ? " · token-billed" : ""}</span></div>
        {account.windows.length ? account.windows.map((w) => <div className="quota-window" key={w.name}>
          <div className="quota-line"><span>{w.name}</span><strong>{w.display}</strong></div>
          {!w.unlimited && <div className="budget-track quota-track"><i className={w.remaining <= 0 ? "budget-fill budget-hot" : "budget-fill"} style={{ width: `${w.used + w.remaining > 0 ? (w.used / (w.used + w.remaining)) * 100 : 0}%` }} /></div>}
          {w.resets_at && <div className="quota-reset">resets {formatTime(w.resets_at)}</div>}
        </div>) : <span className="quota-empty">{account.error ? account.error : "no quota windows"}</span>}
      </div>)}
    </div> : <EmptyState title="No accounts" detail="Add a provider to see its balance here." />}
  </Panel>;
}

/// Bills: who paid, by caller key and by agent.
function BillsPanel({ usage }: { usage: UsageData }) {
  return <div className="content-grid content-grid-secondary">
    <Panel eyebrow="BILLS / BY CALLER KEY" title="Per caller key">
      <div className="usage-list">{usage.by_key.length ? usage.by_key.map(([name, value]) => <div className="usage-row" key={name}><div><strong>{name}</strong><span>{value.requests} requests · {formatTokens(value.input_tokens + value.output_tokens)} tokens</span></div><span className="usage-number">${value.cost_usd.toFixed(4)}</span></div>) : <EmptyState title="No keys used" detail="Bills appear after the first call." />}</div>
    </Panel>
    <Panel eyebrow="BILLS / BY AGENT" title="Per agent">
      <div className="usage-list">{usage.by_agent.length ? usage.by_agent.map(([name, value]) => <div className="usage-row" key={name}><div><strong>{name}</strong><span>{value.requests} requests · {formatTokens(value.input_tokens + value.output_tokens)} tokens</span></div><span className="usage-number">${value.cost_usd.toFixed(4)}</span></div>) : <EmptyState title="No agents identified" detail="Agents are named from their User-Agent (claude-code, codex, pi…)." />}</div>
    </Panel>
  </div>;
}

function SettingsPanel({ settings, callerKeys, setCallerKeys, onChanged, notify }: { settings: Settings; callerKeys: CallerKey[]; setCallerKeys: (keys: CallerKey[]) => void; onChanged: () => Promise<void>; notify: (text: string, error?: boolean) => void }) {
  const [form, setForm] = useState(settings);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => { if (loaded) return; getCallerKeys().then((data) => { setCallerKeys(data.keys); setLoaded(true); }); }, [loaded, setCallerKeys]);
  const save = async () => { try { await saveSettings(form); notify("Settings saved"); await onChanged(); } catch (error) { notify((error as Error).message, true); } };
  const addKey = async () => { const name = window.prompt("Name this caller key"); if (!name) return; try { await api("/api/caller-keys", { method: "POST", body: { name } }); const data = await getCallerKeys(); setCallerKeys(data.keys); notify("Caller key created"); } catch (error) { notify((error as Error).message, true); } };
  return <div className="page-grid"><div className="section-toolbar"><div><span className="eyebrow">SYSTEM / LOCAL CONFIGURATION</span><p>These settings are stored on this machine and hot-reloaded by the gateway.</p></div><Button variant="primary" onClick={save}>Save settings</Button></div><div className="settings-grid"><Panel title="Network"><div className="form-grid"><Field label="Gateway address"><input value={form.gateway_addr} onChange={(event) => setForm({ ...form, gateway_addr: event.target.value })} /></Field><Field label="Gateway port"><input type="number" value={form.gateway_port} onChange={(event) => setForm({ ...form, gateway_port: Number(event.target.value) })} /></Field><Field label="Web address"><input value={form.web_addr} onChange={(event) => setForm({ ...form, web_addr: event.target.value })} /></Field><Field label="Web port"><input type="number" value={form.web_port} onChange={(event) => setForm({ ...form, web_port: Number(event.target.value) })} /></Field></div><Toggle label="Share on local network" checked={form.lan} onChange={(checked) => setForm({ ...form, lan: checked })} /><Toggle label="Send agent User-Agent upstream" checked={form.send_agent_user_agent} onChange={(checked) => setForm({ ...form, send_agent_user_agent: checked })} /><Toggle label="Record usage telemetry" checked={form.record_usage} onChange={(checked) => setForm({ ...form, record_usage: checked })} /></Panel><Panel title="Caller access"><Field label="Default API key"><input value={form.default_caller_key} onChange={(event) => setForm({ ...form, default_caller_key: event.target.value })} /></Field><div className="key-list">{callerKeys.map((key) => <div className="caller-key" key={key.id}><div><strong>{key.name}</strong><span>{key.key}</span></div><StatusDot active={key.enabled} /></div>)}</div><Button variant="secondary" icon="plus" onClick={addKey}>Create caller key</Button></Panel></div></div>;
}

function Metric({ label, value, danger = false }: { label: string; value: string; danger?: boolean }) { return <div className={`metric-box ${danger ? "metric-danger" : ""}`}><span>{label}</span><strong>{value}</strong></div>; }
function Field({ label, children }: { label: string; children: React.ReactNode }) { return <label className="field"><span>{label}</span>{children}</label>; }
function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (checked: boolean) => void }) { return <label className="toggle"><input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} /><span className="toggle-track"><i /></span><span>{label}</span></label>; }
function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: React.ReactNode }) { return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}><div className="modal-card"><div className="modal-header"><div><span className="eyebrow">LOCAL-ROUTER / ACTION</span><h2>{title}</h2></div><button className="modal-close" onClick={onClose}>×</button></div>{children}</div></div>; }
