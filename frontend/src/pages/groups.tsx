import { useState } from "react";
import { useRouteLoaderData } from "react-router";
import { Plus } from "lucide-react";
import { toast } from "sonner";
import type { AppData } from "@/App";
import type { Group } from "@/types";
import { api } from "@/api";
import {
  Badge, Button, Card, Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  EmptyState, Field, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@/components/ui";
import { SectionToolbar, StatusDot } from "@/components/shared/bits";

const routingOptions = [
  { value: "order", label: "order — primary then failover" },
  { value: "rotate", label: "rotate — round robin" },
  { value: "usage", label: "usage — least used" },
  { value: "smart", label: "smart — quota-aware" },
  { value: "weight", label: "weight — weighted interleave" },
];

const staysOptions = [
  { value: "auto", label: "auto — preserve useful cache" },
  { value: "session", label: "session — keep the session" },
  { value: "turn", label: "turn — keep the turn" },
  { value: "off", label: "off — no affinity" },
  { value: "strict", label: "strict — never fail over" },
];

export default function GroupsPage() {
  const { data, reload } = useRouteLoaderData("shell") as { data: AppData; reload: () => Promise<void> };
  const [showAdd, setShowAdd] = useState(false);
  const [form, setForm] = useState({ name: "", models: "", routing: "order", stays: "auto" });

  const create = async () => {
    try {
      await api("/api/groups", {
        method: "POST",
        body: {
          name: form.name,
          models: form.models.split(",").map((item) => item.trim()).filter(Boolean),
          routing: form.routing,
          stays: form.stays,
          listed: true,
        },
      });
      toast.success("Routing group added");
      setShowAdd(false);
      setForm({ name: "", models: "", routing: "order", stays: "auto" });
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };
  const remove = async (group: Group) => {
    if (!window.confirm(`Remove ${group.name}?`)) return;
    try {
      await api(`/api/groups/${encodeURIComponent(group.id)}`, { method: "DELETE" });
      toast.success("Routing group removed");
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="grid gap-5 pt-7">
      <SectionToolbar
        title="Router / policy sets"
        detail="Turn several upstream models into one resilient endpoint."
        action={
          <Button variant="default" onClick={() => setShowAdd(true)}>
            <Plus /> Add routing group
          </Button>
        }
      />
      {data.groups.length ? (
        <div className="grid grid-cols-2 gap-3 max-[900px]:grid-cols-1">
          {data.groups.map((group) => (
            <Card key={group.id} className="p-4.5">
              <div className="flex items-start justify-between gap-4">
                <div>
                  <span className="font-mono text-[10px] text-ring">group/{group.id}</span>
                  <h3 className="m-0 mt-2 text-lg tracking-[-0.04em]">{group.name}</h3>
                </div>
                <Badge variant="primary">{group.routing}</Badge>
              </div>
              <div className="my-5 mb-3 flex items-center gap-2">
                <span className="eyebrow">stays</span>
                <strong className="font-mono text-[11px]">{group.stays}</strong>
                <span className="eyebrow ml-auto">members</span>
                <strong className="font-mono text-[11px]">{group.models.length.toString().padStart(2, "0")}</strong>
              </div>
              <div className="border-t border-border">
                {group.models.map((model, index) => (
                  <div key={model} className="flex items-center gap-2.5 border-b border-border py-3 font-mono text-[11px] last:border-b-0">
                    <i className="not-italic text-muted-foreground/70">{String(index + 1).padStart(2, "0")}</i>
                    <span className="truncate">{model}</span>
                    {index === 0 && group.routing === "order" && (
                      <em className="ml-auto font-mono text-[9px] not-italic text-ring">PRIMARY</em>
                    )}
                  </div>
                ))}
              </div>
              <div className="mt-3 flex items-center justify-between border-t border-border pt-3">
                <span className="flex items-center gap-2 font-mono text-[10px] text-muted-foreground">
                  <StatusDot /> route ready
                </span>
                <Button variant="destructive" size="sm" onClick={() => remove(group)}>Remove</Button>
              </div>
            </Card>
          ))}
        </div>
      ) : (
        <Card>
          <EmptyState title="No routing groups" detail="Create a group to add failover or rotation." />
        </Card>
      )}

      <Dialog open={showAdd} onOpenChange={setShowAdd}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Create routing group</DialogTitle>
          </DialogHeader>
          <div className="grid gap-4 px-5 py-5">
            <Field label="Group name">
              <Input autoFocus value={form.name} placeholder="Daily fallback" onChange={(event) => setForm({ ...form, name: event.target.value })} />
            </Field>
            <Field label="Members" help={`Available now: ${data.models.slice(0, 3).map((m) => m.id).join(" · ")}`}>
              <Input value={form.models} placeholder="provider/model, provider/model" onChange={(event) => setForm({ ...form, models: event.target.value })} />
            </Field>
            <Field label="Routing">
              <Select value={form.routing} onValueChange={(value) => setForm({ ...form, routing: value })}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {routingOptions.map((option) => (
                    <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
            <Field label="Session affinity">
              <Select value={form.stays} onValueChange={(value) => setForm({ ...form, stays: value })}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  {staysOptions.map((option) => (
                    <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          </div>
          <DialogFooter>
            <Button variant="secondary" onClick={() => setShowAdd(false)}>Cancel</Button>
            <Button variant="default" onClick={create}>Create group</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
