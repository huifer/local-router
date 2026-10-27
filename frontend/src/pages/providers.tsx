import { useState } from "react";
import { useRouteLoaderData } from "react-router";
import { LogIn, Plus, Search } from "lucide-react";
import { toast } from "sonner";
import type { AppData } from "@/App";
import type { Preset } from "@/types";
import { api, getPresets } from "@/api";
import {
  Badge, Button, Card, Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  EmptyState, Field, Input,
} from "@/components/ui";
import { SectionToolbar, StatusDot } from "@/components/shared/bits";

export default function ProvidersPage() {
  const { data, reload } = useRouteLoaderData("shell") as { data: AppData; reload: () => Promise<void> };
  const [showAdd, setShowAdd] = useState(false);
  const [presets, setPresets] = useState<{ group: string; presets: Preset[] }[]>([]);
  const [selected, setSelected] = useState<Preset | null>(null);
  const [search, setSearch] = useState("");
  const [signingIn, setSigningIn] = useState(false);
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
      const flow = await api<{ url: string }>(`/api/providers/${encodeURIComponent(presetId)}/login`, {
        method: "POST",
      });
      window.open(flow.url, "_blank", "noopener");
      toast.success("A browser window opened — finish the sign-in there");
      // Poll the vendor until it says the session is good.
      const deadline = Date.now() + 5 * 60 * 1000;
      while (Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 2000));
        try {
          const status = await api<{ done: boolean; waiting: boolean; account?: string }>(
            `/api/providers/${encodeURIComponent(presetId)}/login/status`,
          );
          if (status.done) {
            toast.success(`Signed in${status.account ? ` as ${status.account}` : ""}`);
            setShowAdd(false);
            await reload();
            return;
          }
        } catch {
          // keep waiting; the vendor may still be signing in
        }
      }
      toast.error("the sign-in timed out — try again");
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
      toast.success("A browser window opened — finish the sign-in there");
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
      toast.success("Provider added");
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
    if (!window.confirm(`Remove ${provider.name}?`)) return;
    try {
      await api(`/api/providers/${encodeURIComponent(provider.id)}`, { method: "DELETE" });
      toast.success("Provider removed");
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="grid gap-5 pt-7">
      <SectionToolbar
        title="Registry / upstreams"
        detail="Each provider is an isolated route into your model network."
        action={
          <Button variant="default" onClick={openAdd}>
            <Plus /> Add provider
          </Button>
        }
      />
      {data.providers.length ? (
        <div className="grid grid-cols-3 gap-3 max-[1100px]:grid-cols-2 max-[700px]:grid-cols-1">
          {data.providers.map((provider) => (
            <Card key={provider.id} className="p-4">
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
                {provider.base_url ?? provider.anthropic_base_url ?? "No endpoint"}
              </div>
              <div className="flex gap-3 border-y border-border py-2.5 font-mono text-[10px] text-muted-foreground">
                <span>{provider.auth ? "登录态" : `${provider.keys.length} key${provider.keys.length === 1 ? "" : "s"}`}</span>
                <span>{provider.models.length} models</span>
                {provider.media_models?.length ? <span>{provider.media_models.length} media</span> : null}
                <span className={provider.enabled ? "state-on" : "state-off"}>
                  {provider.enabled ? "ACTIVE" : "PAUSED"}
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
              <div className="flex items-center justify-between gap-2 border-t border-border pt-3">
                <Button variant="ghost" size="sm" onClick={() => toggle(provider)}>
                  {provider.enabled ? "Pause" : "Enable"}
                </Button>
                <Button variant="destructive" size="sm" onClick={() => remove(provider)}>
                  Remove
                </Button>
              </div>
            </Card>
          ))}
        </div>
      ) : (
        <Card>
          <EmptyState
            title="No providers yet"
            detail="Add an upstream so agents have somewhere to send requests."
          />
        </Card>
      )}

      <Dialog open={showAdd} onOpenChange={setShowAdd}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>Add provider</DialogTitle>
          </DialogHeader>
          <div className="grid min-h-80 grid-cols-[245px_1fr] max-[700px]:grid-cols-1">
            <div className="flex flex-col border-r border-border max-[700px]:border-r-0 max-[700px]:border-b">
              <div className="border-b border-border p-3">
                <div className="flex items-center gap-2 border border-input bg-[#0c1110] px-2.5 text-muted-foreground focus-within:border-ring">
                  <Search className="size-3.5 shrink-0" />
                  <Input
                    autoFocus
                    className="h-8 border-0 bg-transparent px-0 focus-visible:ring-0"
                    placeholder="Search vendors…"
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
                    No vendor matches “{search}”.<br />
                    Switch to the form on the right to add it by hand.
                  </p>
                )}
              </div>
            </div>
            <div className="grid content-start gap-4 p-5">
              <Field label="Name">
                <Input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} />
              </Field>
              <Field label="Base URL">
                <Input value={form.url} onChange={(event) => setForm({ ...form, url: event.target.value })} />
              </Field>
              {selected?.session_login ? (
                <div className="grid gap-2 border border-primary/25 bg-primary/5 p-3">
                  <strong className="text-[11px] text-primary">
                    Sign in with {selected.name}
                  </strong>
                  <p className="text-[10px] text-muted-foreground">
                    No API key needed: a browser window opens, you sign in with the vendor, and
                    the session is kept on this machine (auto-renewed).
                  </p>
                  <Button
                    variant="default"
                    onClick={() => sessionSignIn(selected.id)}
                    disabled={signingIn}
                  >
                    <LogIn /> {signingIn ? "Waiting for the sign-in…" : `Sign in with ${selected.name}`}
                  </Button>
                </div>
              ) : selected?.oauth ? (
                <div className="grid gap-2 border border-primary/25 bg-primary/5 p-3">
                  <strong className="text-[11px] text-primary">This vendor signs in with a browser</strong>
                  <p className="text-[10px] text-muted-foreground">
                    No API key needed: a browser window opens, you sign in with the vendor, and the
                    credential is stored on this machine.
                  </p>
                  <Button variant="default" onClick={() => signIn(selected.id)} disabled={signingIn}>
                    <LogIn /> {signingIn ? "Opening…" : "Sign in with " + selected.name}
                  </Button>
                </div>
              ) : (
                <Field label="API key">
                  <Input type="password" value={form.key} onChange={(event) => setForm({ ...form, key: event.target.value })} />
                </Field>
              )}
              <Field label="Exposed models" help="Comma-separated model IDs. Leave empty and refresh the catalog later — auto_include fills it in.">
                <Input value={form.models} onChange={(event) => setForm({ ...form, models: event.target.value })} />
              </Field>
            </div>
          </div>
          <DialogFooter>
            <Button variant="secondary" onClick={() => setShowAdd(false)}>Cancel</Button>
            <Button variant="default" onClick={create}>Add provider</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
