import { useState } from "react";
import { useRouteLoaderData } from "react-router";
import { Plus } from "lucide-react";
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
  const [form, setForm] = useState({ name: "", url: "", key: "", models: "" });

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
            <div className="overflow-auto border-r border-border p-4 max-[700px]:max-h-52 max-[700px]:border-r-0 max-[700px]:border-b">
              {presets.map((group) => (
                <div key={group.group}>
                  <span className="eyebrow mb-2 block">{group.group}</span>
                  {group.presets.map((preset) => (
                    <button
                      key={preset.id}
                      onClick={() => choose(preset)}
                      className={`mb-1 grid w-full cursor-pointer gap-1 border border-transparent p-2.5 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
                        selected?.id === preset.id
                          ? "border-input bg-accent text-foreground"
                          : "border-transparent text-muted-foreground hover:border-border hover:bg-accent hover:text-foreground"
                      }`}
                    >
                      <strong className="text-[11px] font-semibold">{preset.name}</strong>
                      <small className="font-mono text-[9px] text-muted-foreground">
                        {preset.protocol} · {preset.key_hint}
                      </small>
                    </button>
                  ))}
                </div>
              ))}
            </div>
            <div className="grid content-start gap-4 p-5">
              <Field label="Name">
                <Input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} />
              </Field>
              <Field label="Base URL">
                <Input value={form.url} onChange={(event) => setForm({ ...form, url: event.target.value })} />
              </Field>
              <Field label="API key">
                <Input type="password" value={form.key} onChange={(event) => setForm({ ...form, key: event.target.value })} />
              </Field>
              <Field label="Exposed models" help="Comma-separated model IDs. Refresh the catalog later from the CLI.">
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
