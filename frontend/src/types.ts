export type Protocol = "openai_chat" | "openai_responses" | "anthropic" | "gemini" | "qwenwork";

export interface Settings {
  gateway_port: number;
  web_port: number;
  lan: boolean;
  gateway_addr: string;
  web_addr: string;
  default_caller_key: string;
  send_agent_user_agent: boolean;
  masking: boolean;
  record_usage: boolean;
  locale: string;
  /** Origins a browser may call the gateway from. Empty means none. */
  cors_origins: string[];
  /** Claim the WorkBuddy account's daily check-in automatically. */
  workbuddy_auto_checkin: boolean;
}

export interface Status {
  version: string;
  providers: number;
  groups: number;
  models: number;
  settings: Settings;
}

export interface ProviderKey {
  id: string;
  key: string;
  enabled: boolean;
  label?: string;
  max_concurrency?: number | null;
}

export interface ProviderAccount {
  name: string;
  nickname?: string;
  uid?: string;
  type?: string;
  email?: string;
  active?: boolean;
  expires_at?: number | null;
}

export interface Provider {
  id: string;
  name: string;
  kind: string;
  enabled: boolean;
  protocol: Protocol;
  base_url?: string;
  anthropic_base_url?: string;
  responses_base_url?: string;
  gemini_base_url?: string;
  keys: ProviderKey[];
  models: string[];
  auto_include: string[];
  catalog?: string;
  group_only: boolean;
  /** 每个请求都带的静态头；{version}/{platform}/{arch} 会展开。 */
  headers?: Record<string, string>;
  user_agent?: string;
  /** 把调用方发来的同名头原样转发给上游。 */
  forward_headers?: string[];
  /** 发送前删除的头。 */
  remove_headers?: string[];
  /** 上游头的发送顺序（提示性的）。 */
  header_order?: string[];
  /** Where the credential comes from; a local app's login session. */
  auth?: "workbuddy" | "qwenwork";
  chat_path?: string;
  /** The upstream only answers streaming requests. */
  stream_only?: boolean;
  /** The upstream is held back while its quota window is empty. */
  hold_when_exhausted?: boolean;
  /** Provider-level in-flight request limit; API keys can override it. */
  max_concurrency?: number | null;
  /** The models this provider offers for image and video work. */
  media_models?: string[];
  /** For a session-backed provider: who is signed in. */
  account?: ProviderAccount;
}

export type Routing = "smart" | "order" | "rotate" | "usage" | "weight";
export type Stays = "auto" | "session" | "turn" | "off" | "strict";

export interface Group {
  id: string;
  name: string;
  models: string[];
  routing: Routing;
  stays: Stays;
  listed: boolean;
  note?: string;
}

export interface ModelItem {
  id: string;
  provider: string;
  provider_name: string;
  model: string;
  enabled: boolean;
  group?: boolean;
}

export interface Preset {
  id: string;
  name: string;
  protocol: Protocol;
  base_url?: string;
  catalog?: string;
  default_models: string[];
  key_hint?: string;
  docs?: string;
  /** The vendor supports a browser sign-in (OAuth), so no key is needed. */
  oauth?: boolean;
  /**
   * The vendor keeps its own browser session instead of a key:
   * `workbuddy` | `qwenwork`.
   */
  session_login?: "workbuddy" | "qwenwork";
}

export interface PresetGroup {
  group: string;
  presets: Preset[];
}

export interface UsageTotals {
  requests: number;
  failed: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  cost_usd: number;
  credits: number;
  unpriced: number;
}

export interface UsageRecord {
  time: string;
  model: string;
  provider: string;
  key_id?: string;
  key_label?: string;
  caller_key_id?: string;
  caller_key_label?: string;
  group?: string;
  protocol: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  latency_ms: number;
  status: number;
  ok: boolean;
  error?: string;
  agent?: string;
  session_id?: string;
  project?: string;
  cost_usd?: number | null;
  credits?: number | null;
  priced?: boolean;
}

export type UsageStorage = "sqlite" | "jsonl";

export interface SessionRow {
  id: string;
  title: string;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  credits: number;
  failed: number;
  last_seen: string;
}

export interface ProjectRow {
  id: string;
  name: string;
  color: string;
  created_at: string;
  budget_tokens?: number | null;
  budget_cost?: number | null;
  requests: number;
  failed: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  credits: number;
  last_seen: string;
}

export interface TimelinePoint {
  hour: string;
  requests: number;
}

export interface ContextProviderRow {
  provider: string;
  requests: number;
  avg_fill: number;
  system_tokens: number;
  tool_tokens: number;
  file_tokens: number;
  conversation_tokens: number;
  cache_read_tokens: number;
}

export interface QuotaWindow {
  name: string;
  used: number;
  remaining: number;
  resets_at: string | null;
  display: string;
  unit: string;
  unlimited: boolean;
  holds_why?: string | null;
}

export interface QuotaAccount {
  provider: string;
  name: string;
  /** The pool slot this row is about, when the provider keeps a pool. */
  account?: string;
  kind: string;
  user?: string;
  windows: QuotaWindow[];
  unlimited: boolean;
  error?: string;
  read_at?: string;
}

export interface UsageData {
  storage?: UsageStorage;
  currency?: string;
  totals: UsageTotals;
  by_model: [string, UsageTotals][];
  by_provider_costs: Record<string, UsageTotals>;
  by_provider: [string, UsageTotals][];
  by_day: [string, UsageTotals][];
  by_agent: [string, UsageTotals][];
  by_key: [string, UsageTotals][];
  by_provider_key: [string, UsageTotals][];
  by_caller_key: [string, UsageTotals][];
  by_project: [string, UsageTotals][];
  by_session: SessionRow[];
  requests_timeline: TimelinePoint[];
  context: {
    by_provider: ContextProviderRow[];
    totals: { system_tokens: number; tool_tokens: number; file_tokens: number; conversation_tokens: number };
    captured: boolean;
  };
  quota: QuotaAccount[];
  recent: UsageRecord[];
}

export interface CallerKey {
  id: string;
  name: string;
  key: string;
  enabled: boolean;
  daily_tokens?: number;
  monthly_cost?: number;
}

export interface WorkBuddyCredits {
  remaining: number;
  capacity: number;
  packages: {
    package_name: string;
    capacity: number;
    used: number;
    remaining: number;
    unit: string;
    expires_at: number | null;
  }[];
  checkin: {
    checked_in_today: boolean;
    streak_days: number;
    daily_credit: number;
    today_credit: number;
  } | null;
}

export interface AccountView {
  provider: string;
  vendor: string;
  kind: string;
  signed_in?: boolean;
  account?: { nickname?: string; uid?: string; type?: string; email?: string };
  token_expires_at?: number | null;
  plan?: string;
  balance?: string;
  resets_at?: string | null;
  pools?: { name: string; balance: number }[];
  credits?: WorkBuddyCredits;
  error?: string;
}

export interface WorkBuddyStatus {
  signed_in: boolean;
  account: { uid: string; nickname: string; type: string } | null;
  /** The pool slot of the account in use (the one the buttons act on). */
  account_name?: string;
  token_expires_at: number | null;
  credits: WorkBuddyCredits | null;
  note: string | null;
}

/** QwenWork's plan balance and wallet pools, as the dashboard reads it. */
export interface QwenAccountQuota {
  plan: string;
  balance: string;
  resets_at?: string | null;
  pools: { name: string; balance: number }[];
  display: string;
  empty: boolean;
}

/** The live quota of one saved account, keyed by provider kind. */
export interface ProviderAccountQuota {
  kind: "workbuddy" | "qwenwork" | "trae-cn";
  credits?: WorkBuddyCredits;
  quota?: QwenAccountQuota;
}
