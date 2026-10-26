import { useCallback, useEffect, useMemo, useState } from "react";
import { api, getCallerKeys, getGroups, getModels, getPresets, getProviders, getStatus, getUsage, saveSettings } from "./api";
import type { CallerKey, Group, ModelItem, Preset, Provider, Settings, Status, UsageData } from "./types";
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
      {tab === "overview" && <Overview data={data} usage={usage} onOpen={(next) => setTab(next)} />}
      {tab === "providers" && <Providers providers={data.providers} onChanged={reload} notify={notify} />}
      {tab === "models" && <Models models={data.models} />}
      {tab === "groups" && <Groups groups={data.groups} models={data.models} onChanged={reload} notify={notify} />}
      {tab === "usage" && <Usage usage={usage} notify={notify} />}
      {tab === "settings" && <SettingsPanel settings={data.status.settings} callerKeys={callerKeys} setCallerKeys={setCallerKeys} onChanged={reload} notify={notify} />}
    </main>
    {notice && <div className={`notice ${notice.error ? "notice-error" : ""}`}>{notice.text}</div>}
  </div>;
}

function Overview({ data, usage, onOpen }: { data: AppData; usage: UsageData | null; onOpen: (tab: Tab) => void }) {
  const settings = data.status.settings;
  const endpoint = `http://${settings.gateway_addr}:${settings.gateway_port}`;
  const recent = usage?.recent ?? [];
  return <div className="page-grid">
    <div className="signal-row"><div className="signal-card signal-wide"><span className="signal-label">SYSTEM STATUS</span><div className="signal-main"><StatusDot /><strong>Gateway operational</strong><span>All request paths are accepting traffic</span></div><div className="signal-path">{endpoint}</div></div><div className="signal-card"><span className="signal-label">PROVIDERS</span><strong className="metric-value">{data.providers.length.toString().padStart(2, "0")}</strong><span className="metric-detail">configured upstreams</span></div><div className="signal-card"><span className="signal-label">MODELS</span><strong className="metric-value">{data.models.length.toString().padStart(2, "0")}</strong><span className="metric-detail">available endpoints</span></div></div>
    <div className="content-grid content-grid-main"><Panel eyebrow="01 / quick access" title="Operator surface" action={<span className="panel-code">LOCAL / {data.status.version}</span>}><div className="operator-grid"><button onClick={() => onOpen("providers")}><span className="operator-icon"><Icon name="server" /></span><span><strong>Manage providers</strong><small>Keys, endpoints, model catalog</small></span><span className="arrow">↗</span></button><button onClick={() => onOpen("groups")}><span className="operator-icon"><Icon name="activity" /></span><span><strong>Configure routing</strong><small>Failover, rotation, affinity</small></span><span className="arrow">↗</span></button><button onClick={() => onOpen("models")}><span className="operator-icon"><Icon name="layers" /></span><span><strong>Browse model map</strong><small>Every exposed model in one view</small></span><span className="arrow">↗</span></button><button onClick={() => onOpen("usage")}><span className="operator-icon"><Icon name="grid" /></span><span><strong>Inspect usage</strong><small>Tokens, latency, request history</small></span><span className="arrow">↗</span></button></div></Panel><Panel eyebrow="02 / connection" title="Agent environment"><div className="code-block"><div><span className="code-key">OPENAI_BASE_URL</span><span>=</span><span>{endpoint}/v1</span></div><div><span className="code-key">ANTHROPIC_BASE_URL</span><span>=</span><span>{endpoint}</span></div><div><span className="code-key">LOCAL_ROUTER_API_KEY</span><span>=</span><span>{settings.default_caller_key}</span></div></div><div className="panel-footnote"><Icon name="terminal" /> Export these variables to connect any compatible agent.</div></Panel></div>
    <div className="content-grid content-grid-secondary"><Panel eyebrow="03 / live feed" title="Recent requests" action={<button className="text-action" onClick={() => onOpen("usage")}>View all ↗</button>}>{recent.length ? <div className="request-list">{recent.slice(0, 5).map((record, index) => <RequestRow key={`${record.time}-${index}`} record={record} />)}</div> : <EmptyState title="No requests yet" detail="Traffic will appear here when an agent connects." />}</Panel><Panel eyebrow="04 / routing posture" title="Groups at a glance"><div className="group-list">{data.groups.length ? data.groups.slice(0, 4).map((group) => <div className="mini-group" key={group.id}><div><strong>{group.name}</strong><span>{group.models.length} members / {group.routing}</span></div><span className="group-id">group/{group.id}</span></div>) : <EmptyState title="No routing groups" detail="Create a group to add failover or rotation." />}</div></Panel></div>
  </div>;
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
  return <div className="page-grid"><div className="section-toolbar"><div><span className="eyebrow">REGISTRY / UPSTREAMS</span><p>Each provider is an isolated route into your model network.</p></div><Button variant="primary" icon="plus" onClick={openAdd}>Add provider</Button></div><div className="provider-grid">{providers.map((provider) => <article className="provider-card" key={provider.id}><div className="provider-top"><div className="provider-icon"><Icon name="server" /></div><div className="provider-title"><strong>{provider.name}</strong><span>{provider.id} / {provider.protocol}</span></div><StatusDot active={provider.enabled} /></div><div className="provider-url">{provider.base_url ?? provider.anthropic_base_url ?? "No endpoint"}</div><div className="provider-meta"><span>{provider.keys.length} key{provider.keys.length === 1 ? "" : "s"}</span><span>{provider.models.length} models</span><span className={provider.enabled ? "state-on" : "state-off"}>{provider.enabled ? "ACTIVE" : "PAUSED"}</span></div><div className="provider-models">{provider.models.slice(0, 4).map((model) => <span key={model}>{model}</span>)}{provider.models.length > 4 && <span>+{provider.models.length - 4} more</span>}</div><div className="card-actions"><Button variant="quiet" onClick={() => toggle(provider)}>{provider.enabled ? "Pause" : "Enable"}</Button><Button variant="danger" icon="trash" onClick={() => remove(provider)}>Remove</Button></div></article>)}</div>{showAdd && <Modal title="Add provider" onClose={() => setShowAdd(false)}><div className="modal-layout"><div className="preset-list">{presets.map((group) => <div key={group.group}><span className="preset-group">{group.group}</span>{group.presets.map((preset) => <button className={selected?.id === preset.id ? "preset-option preset-selected" : "preset-option"} onClick={() => choose(preset)} key={preset.id}><strong>{preset.name}</strong><small>{preset.protocol} · {preset.key_hint}</small></button>)}</div>)}</div><div className="form-stack"><Field label="Name"><input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} /></Field><Field label="Base URL"><input value={form.url} onChange={(event) => setForm({ ...form, url: event.target.value })} /></Field><Field label="API key"><input type="password" value={form.key} onChange={(event) => setForm({ ...form, key: event.target.value })} /></Field><Field label="Exposed models"><input value={form.models} onChange={(event) => setForm({ ...form, models: event.target.value })} /><small className="field-help">Comma-separated model IDs. Refresh the catalog later from the CLI.</small></Field></div></div><div className="modal-footer"><Button variant="secondary" onClick={() => setShowAdd(false)}>Cancel</Button><Button variant="primary" onClick={create}>Add provider</Button></div></Modal>}</div>;
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

function Usage({ usage, notify }: { usage: UsageData | null; notify: (text: string, error?: boolean) => void }) {
  if (!usage) return <div className="skeleton-page"><div /><div /><div /></div>;
  const totals = usage.totals;
  return <div className="page-grid"><div className="metric-strip"><Metric label="REQUESTS" value={totals.requests.toLocaleString()} /><Metric label="FAILED" value={totals.failed.toLocaleString()} danger={totals.failed > 0} /><Metric label="INPUT TOKENS" value={formatTokens(totals.input_tokens)} /><Metric label="OUTPUT TOKENS" value={formatTokens(totals.output_tokens)} /><Metric label="CACHE READ" value={formatTokens(totals.cache_read_tokens)} /></div><div className="content-grid content-grid-secondary"><Panel eyebrow="USAGE / BY MODEL" title="Model distribution"><div className="usage-list">{usage.by_model.length ? usage.by_model.map(([name, value]) => <div className="usage-row" key={name}><div><strong>{name}</strong><span>{value.requests} requests · {formatTokens(value.output_tokens)} output</span></div><span className="usage-number">{value.requests}</span></div>) : <EmptyState title="No usage data" detail="Request data appears after the first call." />}</div></Panel><Panel eyebrow="USAGE / BY DAY" title="Daily activity"><div className="day-bars">{usage.by_day.length ? usage.by_day.slice(-10).map(([day, value]) => <div className="day-bar" key={day}><div className="bar-track"><i style={{ height: `${Math.max(8, Math.min(100, value.requests * 12))}%` }} /></div><span>{day.slice(5)}</span></div>) : <EmptyState title="No daily activity" detail="The activity graph will populate automatically." />}</div></Panel></div><Panel eyebrow="USAGE / REQUEST LOG" title="Recent requests" action={<Button variant="quiet" onClick={() => getUsage().then(() => notify("Usage refreshed"))}>Refresh log</Button>}>{usage.recent.length ? <div className="request-list">{usage.recent.map((record, index) => <RequestRow key={`${record.time}-${index}`} record={record} />)}</div> : <EmptyState title="No requests yet" detail="Connect an agent to start collecting telemetry." />}</Panel></div>;
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
