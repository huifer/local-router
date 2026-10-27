import { useEffect, useState } from "react";
import { useRouteLoaderData } from "react-router";
import { Plus } from "lucide-react";
import { toast } from "sonner";
import type { AppData } from "@/App";
import type { CallerKey, Settings } from "@/types";
import { api, getCallerKeys, saveSettings } from "@/api";
import {
  Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Field, Input, Switch,
} from "@/components/ui";
import { SectionToolbar, StatusDot } from "@/components/shared/bits";

export default function SettingsPage() {
  const { data, reload } = useRouteLoaderData("shell") as { data: AppData; reload: () => Promise<void> };
  const [form, setForm] = useState<Settings>(data.status.settings);
  const [callerKeys, setCallerKeys] = useState<CallerKey[]>([]);

  useEffect(() => {
    getCallerKeys().then((resp) => setCallerKeys(resp.keys)).catch(() => {});
  }, []);

  const save = async () => {
    try {
      await saveSettings(form);
      toast.success("Settings saved");
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };
  const addKey = async () => {
    const name = window.prompt("Name this caller key");
    if (!name) return;
    try {
      await api("/api/caller-keys", { method: "POST", body: { name } });
      const resp = await getCallerKeys();
      setCallerKeys(resp.keys);
      toast.success("Caller key created");
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="grid gap-5 pt-7">
      <SectionToolbar
        title="System / local configuration"
        detail="These settings are stored on this machine and hot-reloaded by the gateway."
        action={<Button variant="default" onClick={save}>Save settings</Button>}
      />
      <div className="grid grid-cols-2 gap-5 max-[900px]:grid-cols-1">
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">Network</CardDescription>
            <CardTitle>Where the gateway listens</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4">
            <div className="grid grid-cols-2 gap-3.5 max-[500px]:grid-cols-1">
              <Field label="Gateway address">
                <Input value={form.gateway_addr} onChange={(event) => setForm({ ...form, gateway_addr: event.target.value })} />
              </Field>
              <Field label="Gateway port" help="The console shares this port.">
                <Input type="number" value={form.gateway_port} onChange={(event) => setForm({ ...form, gateway_port: Number(event.target.value) })} />
              </Field>
            </div>
            <SettingRow label="Share on local network" detail="Bind 0.0.0.0 so other machines can reach the gateway.">
              <Switch checked={form.lan} onCheckedChange={(checked) => setForm({ ...form, lan: checked })} />
            </SettingRow>
            <SettingRow label="Send agent User-Agent upstream" detail="Some relays require the caller's own identity.">
              <Switch checked={form.send_agent_user_agent} onCheckedChange={(checked) => setForm({ ...form, send_agent_user_agent: checked })} />
            </SettingRow>
            <SettingRow label="Record usage telemetry" detail="Every request lands in usage.jsonl and SQLite.">
              <Switch checked={form.record_usage} onCheckedChange={(checked) => setForm({ ...form, record_usage: checked })} />
            </SettingRow>
            <SettingRow label="Mask secrets in prompts" detail="sk-…, long hex and key-named tool fields are masked before leaving the machine.">
              <Switch checked={form.masking} onCheckedChange={(checked) => setForm({ ...form, masking: checked })} />
            </SettingRow>
            <SettingRow label="WorkBuddy auto check-in" detail="Claim the daily credits without opening the app.">
              <Switch checked={form.workbuddy_auto_checkin} onCheckedChange={(checked) => setForm({ ...form, workbuddy_auto_checkin: checked })} />
            </SettingRow>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">Caller access</CardDescription>
            <CardTitle>Keys other machines use</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4">
            <Field label="Default API key" help="Used when a caller sends no key at all.">
              <Input value={form.default_caller_key} onChange={(event) => setForm({ ...form, default_caller_key: event.target.value })} />
            </Field>
            {callerKeys.length ? (
              <div className="border border-border">
                {callerKeys.map((key) => (
                  <div key={key.id} className="flex items-center justify-between border-b border-border p-3 last:border-b-0">
                    <div>
                      <strong className="block text-[11px]">{key.name}</strong>
                      <span className="mt-0.5 block font-mono text-[10px] text-muted-foreground">{key.key}</span>
                    </div>
                    <StatusDot active={key.enabled} />
                  </div>
                ))}
              </div>
            ) : (
              <p className="font-mono text-[10px] text-muted-foreground/70">
                No caller keys yet — any machine with the default key already works.
              </p>
            )}
            <Button variant="secondary" onClick={addKey}>
              <Plus /> Create caller key
            </Button>
            <Field label="CORS origins" help={'Browser origins allowed to call the gateway. Empty = none. ["*"] = any.'}>
              <Input
                value={(form.cors_origins ?? []).join(", ")}
                placeholder='["http://localhost:3000"]'
                onChange={(event) =>
                  setForm({
                    ...form,
                    cors_origins: event.target.value.split(",").map((item) => item.trim()).filter(Boolean),
                  })
                }
              />
            </Field>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

function SettingRow({ label, detail, children }: { label: string; detail: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 border-t border-border pt-3.5 first:border-t-0 first:pt-0">
      <div>
        <div className="text-xs text-foreground">{label}</div>
        <p className="mt-0.5 text-[10px] text-muted-foreground">{detail}</p>
      </div>
      {children}
    </div>
  );
}
