import { useCallback, useEffect, useState } from "react";
import { useRouteLoaderData } from "react-router";
import { Copy, LogIn, Plus, RefreshCw, Search, X } from "lucide-react";
import { toast } from "sonner";
import type { AppData } from "@/App";
import type {
  Provider,
  ProviderAccount,
  ProviderAccountQuota,
  ProviderKey,
} from "@/types";
import type { Preset } from "@/types";
import { api, getPresets } from "@/api";
import {
  Badge, Button, Card, Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  EmptyState, Field, Input, Sheet, SheetBody, SheetContent, SheetDescription, SheetHeader, SheetTitle,
} from "@/components/ui";
import { useT } from "@/i18n";
import { cn, maskId, maskName } from "@/lib/utils";
import { SectionToolbar, StatusDot } from "@/components/shared/bits";

export default function ProvidersPage() {
  const { t } = useT();
  const { data, reload } = useRouteLoaderData("shell") as { data: AppData; reload: () => Promise<void> };
  const [showAdd, setShowAdd] = useState(false);
  const [presets, setPresets] = useState<{ group: string; presets: Preset[] }[]>([]);
  const [selected, setSelected] = useState<Preset | null>(null);
  const [search, setSearch] = useState("");
  const [signingIn, setSigningIn] = useState(false);
  const [detail, setDetail] = useState<Provider | null>(null);
  const [form, setForm] = useState({ name: "", url: "", key: "", models: "" });

  // Searching the preset list: match the name, the id and the protocol, so
  // "qiniu", "七牛" and "openai_chat" all find their vendor.
  const query = search.trim().toLowerCase();
  const filteredPresets = presets
    .map((group) => ({
      ...group,
      presets: group.presets.filter((preset) =>
        !query ||
        preset.name.toLowerCase().includes(query) ||
        preset.id.toLowerCase().includes(query) ||
        (preset.base_url ?? "").toLowerCase().includes(query) ||
        preset.protocol.toLowerCase().includes(query),
      ),
    }))
    .filter((group) => group.presets.length > 0);

  /**
   * Sign in with a vendor that keeps its own browser session (WorkBuddy,
   * QwenWork): open the vendor's page, then poll until the sign-in lands.
   * The provider is wired up automatically.
   */
  const sessionSignIn = async (presetId: string) => {
    setSigningIn(true);
    try {
      // The gateway creates the provider on sight if it is not configured
      // yet: a session vendor needs no key, so there is nothing else to ask.
      // The gateway picks a free slot name when none is sent, so a second
      // sign-in never overwrites the first.
      const flow = await api<{ url: string; account: string }>(
        `/api/providers/${encodeURIComponent(presetId)}/login`,
        { method: "POST", body: {} },
      );
      window.open(flow.url, "_blank", "noopener");
      toast.success(t("providers.browserOpened"));
      // Poll the vendor until it says the session is good.
      const deadline = Date.now() + 5 * 60 * 1000;
      while (Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 2000));
        try {
          const status = await api<{
            done: boolean;
            waiting: boolean;
            account?: string;
            account_name?: string;
            setup?: { models?: number; checkin?: { granted?: boolean; credit?: number | null } };
          }>(`/api/providers/${encodeURIComponent(presetId)}/login/status`);
          if (status.done) {
            const who = status.account_name ?? status.account ?? "";
            toast.success(who ? t("providers.signedInAs", { name: who }) : t("providers.signedIn"));
            // The gateway already pulled the models and claimed the
            // check-in; say so instead of making the user go look.
            if (status.setup?.models) {
              toast.success(t("providers.setupModels", { n: status.setup.models }));
            }
            if (status.setup?.checkin?.granted) {
              toast.success(t("providers.setupCheckin", { n: status.setup.checkin.credit ?? 0 }));
            }
            setShowAdd(false);
            await reload();
            return;
          }
        } catch {
          // keep waiting; the vendor may still be signing in
        }
      }
      toast.error(t("providers.signInTimeout"));
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setSigningIn(false);
    }
  };

  /** Start the browser sign-in for a vendor that supports OAuth. */
  const signIn = async (presetId: string) => {
    setSigningIn(true);
    try {
      // The provider must exist first: the flow stores the credential on it.
      const existing = data.providers.find((p) => p.id === presetId);
      if (!existing) {
        await api("/api/providers", {
          method: "POST",
          body: { preset: presetId, key: null, models: [] },
        });
      }
      const flow = await api<{ url: string }>(`/api/providers/${encodeURIComponent(presetId)}/oauth`, { method: "POST" });
      window.open(flow.url, "_blank", "noopener");
      toast.success(t("providers.browserOpened"));
      await reload();
      setShowAdd(false);
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setSigningIn(false);
    }
  };

  const openAdd = async () => {
    const response = await getPresets();
    setPresets(response.presets);
    setShowAdd(true);
  };
  const choose = (preset: Preset) => {
    setSelected(preset);
    setForm({ name: preset.name, url: preset.base_url ?? "", key: "", models: preset.default_models.join(", ") });
  };
  const create = async () => {
    try {
      await api("/api/providers", {
        method: "POST",
        body: {
          preset: selected?.id ?? "custom",
          name: form.name,
          base_url: form.url || null,
          key: form.key || null,
          models: form.models.split(",").map((item) => item.trim()).filter(Boolean),
        },
      });
      toast.success(t("providers.added"));
      setShowAdd(false);
      setSelected(null);
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };
  const toggle = async (provider: AppData["providers"][number]) => {
    try {
      await api(`/api/providers/${encodeURIComponent(provider.id)}`, {
        method: "PATCH",
        body: { enabled: !provider.enabled },
      });
      toast.success(`${provider.name} ${provider.enabled ? "paused" : "enabled"}`);
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };
  const remove = async (provider: AppData["providers"][number]) => {
    if (!window.confirm(t("providers.removeConfirm", { name: provider.name }))) return;
    try {
      await api(`/api/providers/${encodeURIComponent(provider.id)}`, { method: "DELETE" });
      toast.success(t("providers.removed"));
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="grid gap-5 pt-7">
      <SectionToolbar
        title={t("providers.eyebrow")}
        detail={t("providers.detail")}
        action={
          <Button variant="default" onClick={openAdd}>
            <Plus /> {t("providers.add")}
          </Button>
        }
      />
      {data.providers.length ? (
        <div className="grid grid-cols-3 gap-3 max-[1100px]:grid-cols-2 max-[700px]:grid-cols-1">
          {data.providers.map((provider) => (
            <Card key={provider.id} className="p-4 transition-colors hover:border-input">
              <button
                onClick={() => setDetail(provider)}
                className="block w-full cursor-pointer text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
                aria-label={`Details of ${provider.name}`}
              >
              <div className="flex items-center gap-2.5">
                <span className="grid size-8 shrink-0 place-items-center border border-input text-primary">▣</span>
                <div className="min-w-0 flex-1">
                  <strong className="block truncate text-xs">{provider.name}</strong>
                  <span className="mt-0.5 block font-mono text-[10px] text-muted-foreground">
                    {provider.id} / {provider.protocol}
                  </span>
                </div>
                <StatusDot active={provider.enabled} />
              </div>
              <div className="my-3.5 min-h-8 font-mono text-[10px] leading-relaxed text-muted-foreground break-all">
                {provider.base_url ?? provider.anthropic_base_url ?? t("providers.noEndpoint")}
              </div>
              <div className="flex gap-3 border-y border-border py-2.5 font-mono text-[10px] text-muted-foreground">
                <span>{provider.auth ? t("providers.session") : t(provider.keys.length === 1 ? "providers.keys" : "providers.keysPlural", { n: provider.keys.length })}</span>
                {provider.account?.nickname && (
                  <span className="truncate text-primary" title={provider.account.uid}>
                    {maskName(provider.account.nickname)}
                  </span>
                )}
                <span>{t("providers.models", { n: provider.models.length })}</span>
                {provider.media_models?.length ? <span>{t("providers.media", { n: provider.media_models.length })}</span> : null}
                <span className={provider.enabled ? "state-on" : "state-off"}>
                  {provider.enabled ? t("providers.active") : t("providers.paused")}
                </span>
              </div>
              <div className="flex min-h-14 flex-wrap content-start gap-1 py-3">
                {provider.models.slice(0, 4).map((model) => (
                  <Badge key={model}>{model}</Badge>
                ))}
                {provider.models.length > 4 && <Badge>+{provider.models.length - 4} more</Badge>}
                {provider.media_models?.map((model) => (
                  <Badge key={`media-${model}`} variant="primary">{model}</Badge>
                ))}
              </div>
              </button>
              <div className="flex items-center justify-between gap-2 border-t border-border pt-3">
                <Button variant="ghost" size="sm" onClick={() => toggle(provider)}>
                  {provider.enabled ? t("providers.pause") : t("providers.enable")}
                </Button>
                <Button variant="destructive" size="sm" onClick={() => remove(provider)}>
                  {t("providers.remove")}
                </Button>
              </div>
            </Card>
          ))}
        </div>
      ) : (
        <Card>
          <EmptyState
            title={t("providers.empty")}
            detail={t("providers.emptyDetail")}
          />
        </Card>
      )}

      <ProviderDetail provider={detail} onClose={() => setDetail(null)} onChanged={reload} />

      <Dialog open={showAdd} onOpenChange={setShowAdd}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>{t("providers.add")}</DialogTitle>
          </DialogHeader>
          <div className="grid min-h-80 grid-cols-[245px_1fr] max-[700px]:grid-cols-1">
            <div className="flex flex-col border-r border-border max-[700px]:border-r-0 max-[700px]:border-b">
              <div className="border-b border-border p-3">
                <div className="flex items-center gap-2 border border-input bg-[#0c1110] px-2.5 text-muted-foreground focus-within:border-ring">
                  <Search className="size-3.5 shrink-0" />
                  <Input
                    autoFocus
                    className="h-8 border-0 bg-transparent px-0 focus-visible:ring-0"
                    placeholder={t("providers.searchPlaceholder")}
                    value={search}
                    onChange={(event) => setSearch(event.target.value)}
                  />
                </div>
              </div>
              <div className="overflow-auto p-3 max-[700px]:max-h-52">
                {filteredPresets.length ? (
                  filteredPresets.map((group) => (
                    <div key={group.group}>
                      <span className="eyebrow mb-2 block">{group.group}</span>
                      {group.presets.map((preset) => (
                        <button
                          key={preset.id}
                          onClick={() => choose(preset)}
                          className={`mb-1 grid w-full cursor-pointer gap-1 border p-2.5 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
                            selected?.id === preset.id
                              ? "border-input bg-accent text-foreground"
                              : "border-transparent text-muted-foreground hover:border-border hover:bg-accent hover:text-foreground"
                          }`}
                        >
                          <span className="flex items-center gap-1.5">
                            <strong className="text-[11px] font-semibold">{preset.name}</strong>
                            {preset.oauth && <Badge variant="primary">OAUTH</Badge>}
                            {preset.session_login && <Badge variant="primary">LOGIN</Badge>}
                          </span>
                          <small className="font-mono text-[9px] text-muted-foreground">
                            {preset.protocol} · {preset.key_hint}
                          </small>
                        </button>
                      ))}
                    </div>
                  ))
                ) : (
                  <p className="px-2 py-6 text-center font-mono text-[10px] text-muted-foreground/70">
                    {t("providers.noVendorMatch", { q: search })}<br />
                    {t("providers.noVendorMatchDetail")}
                  </p>
                )}
              </div>
            </div>
            <div className="grid content-start gap-4 p-5">
              <Field label={t("providers.name")}>
                <Input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} />
              </Field>
              <Field label={t("providers.baseUrl")}>
                <Input value={form.url} onChange={(event) => setForm({ ...form, url: event.target.value })} />
              </Field>
              {selected?.session_login ? (
                <div className="grid gap-2 border border-primary/25 bg-primary/5 p-3">
                  <strong className="text-[11px] text-primary">
                    {t("providers.sessionTitle", { name: selected.name })}
                  </strong>
                  <p className="text-[10px] text-muted-foreground">{t("providers.sessionDetail")}</p>
                  <Button
                    variant="default"
                    onClick={() => sessionSignIn(selected.id)}
                    disabled={signingIn}
                  >
                    <LogIn /> {signingIn ? t("providers.sessionWaiting") : t("providers.oauthButton", { name: selected.name })}
                  </Button>
                </div>
              ) : selected?.oauth ? (
                <div className="grid gap-2 border border-primary/25 bg-primary/5 p-3">
                  <strong className="text-[11px] text-primary">{t("providers.oauthTitle")}</strong>
                  <p className="text-[10px] text-muted-foreground">{t("providers.oauthDetail")}</p>
                  <Button variant="default" onClick={() => signIn(selected.id)} disabled={signingIn}>
                    <LogIn /> {signingIn ? t("providers.oauthOpening") : t("providers.oauthButton", { name: selected.name })}
                  </Button>
                </div>
              ) : (
                <Field label={t("providers.apiKey")}>
                  <Input type="password" value={form.key} onChange={(event) => setForm({ ...form, key: event.target.value })} />
                </Field>
              )}
              <Field label={t("providers.exposedModelsLabel")} help={t("providers.exposedModelsHelp")}>
                <Input value={form.models} onChange={(event) => setForm({ ...form, models: event.target.value })} />
              </Field>
            </div>
          </div>
          <DialogFooter>
            <Button variant="secondary" onClick={() => setShowAdd(false)}>{t("providers.cancel")}</Button>
            <Button variant="default" onClick={create}>{t("providers.add")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function ProviderDetail({ provider, onClose, onChanged }: { provider: Provider | null; onClose: () => void; onChanged: () => Promise<void> }) {
  const { t } = useT();
  const yesNo = (value: boolean | undefined) => (value ? t("providers.yes") : t("providers.no"));
  return (
    <Sheet open={Boolean(provider)} onOpenChange={(open) => !open && onClose()}>
      <SheetContent>
        {provider && (
          <>
            <SheetHeader>
              <SheetTitle>{provider.name}</SheetTitle>
              <SheetDescription>{provider.id} · {provider.protocol}</SheetDescription>
            </SheetHeader>
            <SheetBody>
              <div className="grid grid-cols-3 gap-2 border-b border-border p-4">
                <DetailStat label={t("providers.status")} value={provider.enabled ? t("providers.active") : t("providers.paused")} accent={provider.enabled} />
                <DetailStat label={t("nav.models")} value={String(provider.models.length)} />
                <DetailStat label={t("providers.keysLabel")} value={provider.auth ? t("providers.session") : String(provider.keys.length)} />
              </div>
              <div className="grid gap-0 px-4 py-2">
                <DetailRow label={t("providers.baseUrl")} value={provider.base_url ?? "—"} />
                <DetailRow label={t("providers.anthropicUrl")} value={provider.anthropic_base_url ?? "—"} />
                <DetailRow label={t("providers.responsesUrl")} value={provider.responses_base_url ?? "—"} />
                <DetailRow label={t("providers.geminiUrl")} value={provider.gemini_base_url ?? "—"} />
                <DetailRow label={t("providers.chatPath")} value={provider.chat_path ?? t("providers.default")} />
                <DetailRow label={t("providers.catalog")} value={provider.catalog ?? "—"} />
                <DetailRow label={t("providers.authKind")} value={provider.auth ? t("providers.credentialSession") : t("providers.credentialKey")} />
                <DetailRow label={t("providers.groupOnly")} value={yesNo(provider.group_only)} />
                <DetailRow label={t("providers.streamOnly")} value={yesNo(provider.stream_only)} />
                <DetailRow label={t("providers.holdWhenExhausted")} value={yesNo(provider.hold_when_exhausted)} />
                <DetailRow label={t("providers.autoInclude")} value={provider.auto_include?.join(", ") || "—"} />
                <DetailRow label={t("providers.forwardHeaders")} value={provider.forward_headers?.join(", ") || "—"} />
                <DetailRow label={t("providers.removeHeaders")} value={provider.remove_headers?.join(", ") || "—"} />
                <DetailRow label={t("providers.userAgent")} value={provider.user_agent || t("providers.default")} />
              </div>
              {!provider.auth ? (
                <KeyList provider={provider} />
              ) : null}
              <div className="border-t border-border p-4">
                <div className="eyebrow mb-2">{t("providers.exposedModels")}</div>
                <div className="flex flex-wrap gap-1.5">
                  {provider.models.length
                    ? provider.models.map((model) => <Badge key={model}>{model}</Badge>)
                    : <span className="font-mono text-[10px] text-muted-foreground">{t("providers.noModels")}</span>}
                </div>
              </div>
              {provider.media_models?.length ? (
                <div className="border-t border-border p-4">
                  <div className="eyebrow mb-2">{t("providers.mediaModels")}</div>
                  <div className="flex flex-wrap gap-1.5">
                    {provider.media_models.map((model) => <Badge key={model} variant="primary">{model}</Badge>)}
                  </div>
                </div>
              ) : null}
              {provider.auth ? (
                <AccountPool provider={provider} onChanged={onChanged} />
              ) : null}
            </SheetBody>
          </>
        )}
      </SheetContent>
    </Sheet>
  );
}

function DetailStat({ label, value, accent = false }: { label: string; value: string; accent?: boolean }) {
  return <div className="border border-border bg-secondary/50 p-2.5"><div className="eyebrow">{label}</div><strong className={`mt-1.5 block font-mono text-base ${accent ? "text-primary" : ""}`}>{value}</strong></div>;
}

/**
 * The provider's stored keys: masked by default, and revealed on demand so
 * they can be copied. The secret is fetched when the user asks for it, so
 * it is not in the page until then.
 */
function KeyList({ provider }: { provider: Provider }) {
  const { t } = useT();
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);

  const reveal = async (key: ProviderKey) => {
    if (revealed[key.id]) {
      setRevealed((all) => {
        const next = { ...all };
        delete next[key.id];
        return next;
      });
      return;
    }
    setBusy(key.id);
    try {
      const response = await api<{ key: string }>(
        `/api/providers/${encodeURIComponent(provider.id)}/keys/${encodeURIComponent(key.id)}/secret`,
      );
      setRevealed((all) => ({ ...all, [key.id]: response.key }));
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const copy = async (key: ProviderKey) => {
    const value = revealed[key.id] ?? key.key;
    try {
      await navigator.clipboard.writeText(value);
      toast.success(t("providers.keyCopied", { name: key.label || key.id }));
    } catch {
      toast.error(t("providers.keyCopyFailed"));
    }
  };

  if (!provider.keys.length) {
    return (
      <div className="border-t border-border p-4">
        <div className="eyebrow mb-2">{t("providers.keysLabel")}</div>
        <p className="font-mono text-[10px] text-muted-foreground">{t("providers.noKeys")}</p>
      </div>
    );
  }

  return (
    <div className="border-t border-border p-4">
      <div className="eyebrow mb-2">{t("providers.keysLabel")}</div>
      <div className="grid gap-1.5">
        {provider.keys.map((key) => {
          const shown = revealed[key.id];
          return (
            <div key={key.id} className="flex items-center gap-2 border border-border bg-secondary/40 px-2.5 py-2">
              <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground">
                {shown ?? key.key}
              </span>
              {key.label ? <Badge className="shrink-0">{key.label}</Badge> : null}
              <Button variant="ghost" size="sm" onClick={() => reveal(key)} disabled={busy === key.id}>
                {shown ? t("providers.keyHide") : t("providers.keyShow")}
              </Button>
              <Button variant="ghost" size="sm" onClick={() => copy(key)}>
                <Copy /> {t("providers.keyCopy")}
              </Button>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function DetailRow({ label, value }: { label: string; value: string }) {
  return <div className="grid grid-cols-[120px_1fr] gap-3 border-b border-border/60 py-2 last:border-b-0"><span className="eyebrow">{label}</span><span className="break-all font-mono text-[11px] text-muted-foreground">{value}</span></div>;
}


/**
 * The accounts behind a session-backed provider: who is signed in, which
 * one is in use, and a way to add another (the sign-in page opens, the
 * gateway does the rest) or switch between them.
 */
function AccountPool({ provider, onChanged }: { provider: Provider; onChanged: () => Promise<void> }) {
  const { t } = useT();
  const [accounts, setAccounts] = useState<ProviderAccount[]>([]);
  const [busy, setBusy] = useState(false);
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState("");
  // The quota of every account, by slot: each one has its own.
  const [quotas, setQuotas] = useState<Record<string, ProviderAccountQuota>>({});
  const [pending, setPending] = useState<Record<string, boolean>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [refreshingAll, setRefreshingAll] = useState(false);

  const load = useCallback(async () => {
    try {
      const response = await api<{ accounts: ProviderAccount[] }>(
        `/api/providers/${encodeURIComponent(provider.id)}/accounts`,
      );
      setAccounts(response.accounts);
    } catch {
      setAccounts([]);
    }
  }, [provider.id]);

  /** Ask the vendor for one account's quota and check-in state. */
  const refreshOne = useCallback(
    async (name: string) => {
      setPending((all) => ({ ...all, [name]: true }));
      setErrors((all) => {
        const next = { ...all };
        delete next[name];
        return next;
      });
      try {
        const response = await api<ProviderAccountQuota>(
          `/api/providers/${encodeURIComponent(provider.id)}/accounts/${encodeURIComponent(name)}/quota`,
        );
        setQuotas((all) => ({ ...all, [name]: response }));
      } catch (err) {
        setErrors((all) => ({ ...all, [name]: (err as Error).message }));
      } finally {
        setPending((all) => ({ ...all, [name]: false }));
      }
    },
    [provider.id],
  );

  /** Bring every account's quota and check-in state up to date. */
  const refreshAll = useCallback(async () => {
    if (!accounts.length) return;
    setRefreshingAll(true);
    await Promise.all(accounts.map((account) => refreshOne(account.name)));
    setRefreshingAll(false);
  }, [accounts, refreshOne]);

  useEffect(() => { load(); }, [load]);

  // Every account's status is read when the pool shows up, so the check-in
  // state of all of them is current without a click per account.
  useEffect(() => {
    if (provider.id === "trae-cn") return;
    if (!accounts.length) return;
    refreshAll();
  }, [accounts, provider.id, refreshAll]);

  /** Sign in another account: the browser opens, the gateway finishes. */
  const add = async () => {
    // An empty name lets the gateway take the first free slot, so a new
    // sign-in never overwrites an account that is already there.
    const name = newName.trim();
    setBusy(true);
    try {
      const flow = await api<{ url: string; account?: string }>(
        `/api/providers/${encodeURIComponent(provider.id)}/login`,
        {
          method: "POST",
          body: name ? { account: name } : {},
        },
      );
      window.open(flow.url, "_blank", "noopener");
      toast.success(t("providers.accountOpened"));
      const deadline = Date.now() + 5 * 60 * 1000;
      while (Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 2000));
        const status = await api<{
          done: boolean;
          account?: string;
          account_name?: string;
          same_uid_elsewhere?: boolean;
        }>(
          `/api/providers/${encodeURIComponent(provider.id)}/login/status`,
        );
        if (status.done) {
          const who = status.account_name ?? flow.account ?? name;
          toast.success(t("providers.accountAdded", { name: who }));
          // The browser signed in whoever was logged in there. If that is
          // the same person as another saved account, say so — the fix is
          // "Switch Account" on the sign-in page, not a new entry.
          if (status.same_uid_elsewhere) {
            toast.info(t("providers.sameUidHint"));
          }
          setAdding(false);
          setNewName("");
          await load();
          await onChanged();
          return;
        }
      }
      toast.error(t("providers.signInTimeout"));
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setBusy(false);
    }
  };

  /** Switch to another account. */
  const use = async (name: string) => {
    try {
      await api(`/api/providers/${encodeURIComponent(provider.id)}/accounts/active`, {
        method: "POST",
        body: { name },
      });
      toast.success(t("providers.accountSwitched", { name }));
      await load();
      await onChanged();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  /** Forget one saved account (its login state is dropped). */
  const remove = async (name: string) => {
    if (!window.confirm(t("providers.removeConfirm", { name }))) return;
    try {
      await api(
        `/api/providers/${encodeURIComponent(provider.id)}/accounts/${encodeURIComponent(name)}`,
        { method: "DELETE" },
      );
      toast.success(t("providers.accountRemoved", { name }));
      await load();
      await onChanged();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="border-t border-border p-4">
      <div className="mb-2 flex items-center justify-between gap-3">
        <span className="eyebrow">{t("providers.accounts")}</span>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="sm"
            title={t("providers.quotaRefreshAll")}
            onClick={refreshAll}
            disabled={refreshingAll || !accounts.length || provider.id === "trae-cn"}
          >
            <RefreshCw className={refreshingAll ? "animate-spin" : ""} /> {t("providers.quotaRefreshAll")}
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setAdding((value) => !value)}>
            <Plus /> {t("providers.addAccount")}
          </Button>
        </div>
      </div>
      {adding && (
        <div className="mb-3 flex items-center gap-2">
          <Input
            autoFocus
            className="h-8"
            placeholder={t("providers.accountNamePlaceholder")}
            value={newName}
            onChange={(event) => setNewName(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Enter") add(); }}
          />
          <Button variant="default" size="sm" onClick={add} disabled={busy}>
            {busy ? t("providers.accountWaiting") : t("providers.signIn")}
          </Button>
        </div>
      )}
      <p className="mb-2 font-mono text-[9px] text-muted-foreground/70">
        {t("providers.addAccountHint")}
      </p>
      {accounts.length ? (
        <div className="grid gap-1.5">
          {accounts.map((account) => (
            <AccountRow
              key={account.name}
              provider={provider}
              account={account}
              quota={quotas[account.name] ?? null}
              loading={Boolean(pending[account.name])}
              error={errors[account.name] ?? null}
              onRefresh={refreshOne}
              onUse={use}
              onRemove={remove}
              onChanged={onChanged}
            />
          ))}
        </div>
      ) : (
        <p className="font-mono text-[10px] text-muted-foreground/70">
          {t("providers.noAccounts")}
        </p>
      )}
    </div>
  );
}

/**
 * One saved account: who it is, whether it is the one in use, and its own
 * quota and check-in state. Every account has an independent quota, so the
 * numbers are read per account and the refresh button asks the vendor again.
 */
function AccountRow({
  provider,
  account,
  quota,
  loading,
  error,
  onRefresh,
  onUse,
  onRemove,
  onChanged,
}: {
  provider: Provider;
  account: ProviderAccount;
  quota: ProviderAccountQuota | null;
  loading: boolean;
  error: string | null;
  onRefresh: (name: string) => Promise<void>;
  onUse: (name: string) => Promise<void>;
  onRemove: (name: string) => Promise<void>;
  onChanged: () => Promise<void>;
}) {
  const { t } = useT();
  const [checking, setChecking] = useState(false);

  /** Claim today's check-in for this account. */
  const checkIn = async () => {
    setChecking(true);
    try {
      const result = await api<{ granted?: boolean; already?: boolean; credit?: number | null; message?: string }>(
        `/api/providers/${encodeURIComponent(provider.id)}/accounts/${encodeURIComponent(account.name)}/checkin`,
        { method: "POST", body: {} },
      );
      if (result.granted) toast.success(t("providers.checkedIn", { n: result.credit ?? 0 }));
      else toast.success(result.message || t("providers.alreadyCheckedIn"));
      await onRefresh(account.name);
      await onChanged();
    } catch (err) {
      toast.error((err as Error).message);
    } finally {
      setChecking(false);
    }
  };

  const credits = quota?.credits;
  const qwen = quota?.quota;
  const hasQuota = provider.id !== "trae-cn";

  return (
    <div
      className={cn(
        "grid gap-2 border px-2.5 py-2",
        account.active ? "border-primary/30 bg-primary/5" : "border-border",
      )}
    >
      <div className="flex items-center gap-2">
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[11px] text-foreground">
            {maskName(account.nickname) || account.name}
          </span>
          <span className="block truncate font-mono text-[9px] text-muted-foreground">
            {account.name}{account.uid ? ` · ${maskId(account.uid)}` : ""}
          </span>
        </span>
        {account.active ? (
          <Badge variant="primary">{t("providers.inUse")}</Badge>
        ) : (
          <Button variant="ghost" size="sm" onClick={() => onUse(account.name)}>
            {t("providers.use")}
          </Button>
        )}
        <Button
          variant="ghost"
          size="sm"
          title={t("providers.removeAccount")}
          onClick={() => onRemove(account.name)}
        >
          <X />
        </Button>
      </div>

      {hasQuota ? (
        <div className="grid gap-2 border-t border-border/60 pt-2">
          <div className="flex items-center gap-2">
            <span className="eyebrow">{t("providers.quota")}</span>
            {loading ? (
              <span className="font-mono text-[9px] text-muted-foreground">…</span>
            ) : credits ? (
              <span className="font-mono text-[10px] text-foreground">
                {t("providers.quotaLeft")}: {credits.remaining} / {credits.capacity}
              </span>
            ) : qwen ? (
              <span className="font-mono text-[10px] text-foreground">
                {t("providers.balance")}: {qwen.display || qwen.balance}
              </span>
            ) : null}
            <Button
              variant="ghost"
              size="sm"
              className="ml-auto"
              title={t("providers.quotaRefresh")}
              onClick={() => onRefresh(account.name)}
              disabled={loading}
            >
              <RefreshCw className={loading ? "animate-spin" : ""} /> {t("providers.quotaRefresh")}
            </Button>
          </div>

          {error ? (
            <p className="font-mono text-[9px] text-destructive">{error}</p>
          ) : null}

          {credits?.packages.length ? (
            <div className="grid gap-1">
              {/* Only the packages that still have something in them — the
                  empty ones are the same story repeated. */}
              {credits.packages
                .filter((item) => item.remaining > 0)
                .slice(0, 4)
                .map((item, index) => (
                  <div
                    key={`${item.package_name}-${index}`}
                    className="grid grid-cols-[1fr_auto] items-center gap-2 font-mono text-[9px] text-muted-foreground"
                  >
                    <span className="truncate">{item.package_name}</span>
                    <span>
                      {item.remaining} / {item.capacity} {item.unit}
                    </span>
                  </div>
                ))}
              {credits.packages.filter((item) => item.remaining > 0).length > 4 ? (
                <span className="font-mono text-[9px] text-muted-foreground/70">
                  +{credits.packages.filter((item) => item.remaining > 0).length - 4} more
                </span>
              ) : null}
            </div>
          ) : null}

          {qwen?.pools.length ? (
            <div className="flex flex-wrap gap-1.5">
              {qwen.pools.map((pool) => (
                <Badge key={pool.name} variant="primary">
                  {pool.name}: {pool.balance}
                </Badge>
              ))}
            </div>
          ) : null}

          {provider.id === "workbuddy" ? (
            <div className="flex items-center gap-2">
              <span className="font-mono text-[9px] text-muted-foreground">
                {t("providers.checkIn")}:{" "}
                {credits
                  ? credits.checkin?.checked_in_today
                    ? t("providers.checkInDone")
                    : t("providers.checkInPending")
                  : "—"}
                {credits?.checkin?.streak_days
                  ? ` · ${credits.checkin.streak_days}d`
                  : ""}
              </span>
              <Button
                variant="secondary"
                size="sm"
                className="ml-auto"
                onClick={checkIn}
                disabled={checking || credits?.checkin?.checked_in_today === true}
              >
                {checking
                  ? "…"
                  : credits?.checkin?.checked_in_today
                    ? t("providers.checkInDone")
                    : t("providers.checkInClaim")}
              </Button>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
