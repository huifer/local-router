export type Protocol = "openai_chat" | "openai_responses" | "anthropic" | "gemini";

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
  /** Where the credential comes from; "workbuddy" means the local session. */
  auth?: "workbuddy";
  chat_path?: string;
  /** The upstream only answers streaming requests. */
  stream_only?: boolean;
  /** The models this provider offers for image and video work. */
  media_models?: string[];
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
}

export interface UsageRecord {
  time: string;
  model: string;
  provider: string;
  key_id?: string;
  key_label?: string;
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
}

export interface UsageData {
  totals: UsageTotals;
  by_model: [string, UsageTotals][];
  by_provider: [string, UsageTotals][];
  by_day: [string, UsageTotals][];
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

export interface WorkBuddyStatus {
  signed_in: boolean;
  account: { uid: string; nickname: string; type: string } | null;
  token_expires_at: number | null;
  credits: WorkBuddyCredits | null;
  note: string | null;
}
