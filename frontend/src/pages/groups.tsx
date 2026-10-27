import { useMemo, useState } from "react";
import { useRouteLoaderData } from "react-router";
import { Check, Plus, Search } from "lucide-react";
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

/** The members a group can be built from: every exposed upstream model. */
function useMemberCatalog() {
  const { data } = useRouteLoaderData("shell") as { data: AppData };
  return useMemo(
    () => data.models.filter((model) => !model.group),
    [data.models],
  );
}

export default function GroupsPage() {
  const { data, reload } = useRouteLoaderData("shell") as { data: AppData; reload: () => Promise<void> };
  const [showAdd, setShowAdd] = useState(false);
  const [form, setForm] = useState({ name: "", members: [] as string[], routing: "order", stays: "auto" });

  const create = async () => {
    if (!form.members.length) {
      toast.error("Pick at least one model for the group");
      return;
    }
    try {
      await api("/api/groups", {
        method: "POST",
        body: {
          name: form.name,
          models: form.members,
          routing: form.routing,
          stays: form.stays,
          listed: true,
        },
      });
      toast.success("Routing group added");
      setShowAdd(false);
      setForm({ name: "", members: [], routing: "order", stays: "auto" });
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
          <Button variant="default" onClick={() => setShowAdd(true)} disabled={!data.models.length}>
            <Plus /> Add routing group
          </Button>
        }
      />

      {!data.models.length ? (
        <Card>
          <EmptyState
            title="No models to group yet"
            detail="A routing group is built from the models your providers expose. Add a provider first, then come back."
          />
        </Card>
      ) : data.groups.length ? (
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
                {group.models.map((model, index) => {
                  const owner = model.split("/")[0];
                  return (
                    <div key={model} className="flex items-center gap-2.5 border-b border-border py-3 font-mono text-[11px] last:border-b-0">
                      <i className="not-italic text-muted-foreground/70">{String(index + 1).padStart(2, "0")}</i>
                      <span className="truncate">{model}</span>
                      <Badge className="ml-auto shrink-0">{owner}</Badge>
                      {index === 0 && group.routing === "order" && (
                        <em className="shrink-0 font-mono text-[9px] not-italic text-ring">PRIMARY</em>
                      )}
                    </div>
                  );
                })}
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
          <EmptyState
            title="No routing groups"
            detail="Pick a few of your exposed models and the gateway will fail over between them."
          />
        </Card>
      )}

      <Dialog open={showAdd} onOpenChange={setShowAdd}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>Create routing group</DialogTitle>
          </DialogHeader>
          <div className="grid gap-4 px-5 py-5">
            <Field label="Group name">
              <Input
                autoFocus
                value={form.name}
                placeholder="Daily fallback"
                onChange={(event) => setForm({ ...form, name: event.target.value })}
              />
            </Field>
            <Field
              label="Members"
              help="Pick from the models your providers already expose. The first one answers first under `order`."
            >
              <MemberPicker
                selected={form.members}
                onChange={(members) => setForm({ ...form, members })}
              />
            </Field>
            <div className="grid grid-cols-2 gap-3.5 max-[500px]:grid-cols-1">
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
          </div>
          <DialogFooter>
            <span className="mr-auto font-mono text-[10px] text-muted-foreground">
              {form.members.length} member{form.members.length === 1 ? "" : "s"} selected
            </span>
            <Button variant="secondary" onClick={() => setShowAdd(false)}>Cancel</Button>
            <Button variant="default" onClick={create} disabled={!form.members.length}>
              Create group
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/**
 * The member picker: the models the providers expose, grouped by provider,
 * with a search box and an ordered selection. Order matters (it is the
 * failover order), so the picked list is numbered and can be reordered by
 * clicking it off and on again.
 */
function MemberPicker({ selected, onChange }: { selected: string[]; onChange: (next: string[]) => void }) {
  const catalog = useMemberCatalog();
  const [query, setQuery] = useState("");

  const byProvider = useMemo(() => {
    const map = new Map<string, { id: string; label: string }[]>();
    for (const model of catalog) {
      const list = map.get(model.provider) ?? [];
      list.push({ id: model.id, label: model.model });
      map.set(model.provider, list);
    }
    return [...map.entries()];
  }, [catalog]);

  const matches = (model: { id: string; label: string }) =>
    !query.trim() ||
    model.id.toLowerCase().includes(query.toLowerCase()) ||
    model.label.toLowerCase().includes(query.toLowerCase());

  const toggle = (id: string) => {
    onChange(selected.includes(id) ? selected.filter((item) => item !== id) : [...selected, id]);
  };

  return (
    <div className="grid gap-2">
      <div className="flex items-center gap-2 border border-input bg-[#0c1110] px-2.5 text-muted-foreground focus-within:border-ring">
        <Search className="size-3.5 shrink-0" />
        <Input
          className="h-8 border-0 bg-transparent px-0 focus-visible:ring-0"
          placeholder="Search models…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </div>
      <div className="max-h-56 overflow-auto border border-border">
        {byProvider.map(([provider, models]) => {
          const visible = models.filter(matches);
          if (!visible.length) return null;
          return (
            <div key={provider} className="border-b border-border last:border-b-0">
              <div className="flex items-center gap-2 bg-secondary/60 px-3 py-2">
                <span className="eyebrow">{provider}</span>
                <span className="font-mono text-[9px] text-muted-foreground/60">{visible.length} models</span>
              </div>
              {visible.map((model) => {
                const index = selected.indexOf(model.id);
                const picked = index !== -1;
                return (
                  <button
                    key={model.id}
                    onClick={() => toggle(model.id)}
                    className={`flex w-full cursor-pointer items-center gap-2.5 border-t border-border/50 px-3 py-2 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
                      picked ? "bg-primary/5 text-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground"
                    }`}
                  >
                    <span
                      className={`grid size-4 shrink-0 place-items-center border ${
                        picked ? "border-primary bg-primary text-primary-foreground" : "border-input"
                      }`}
                    >
                      {picked && <Check className="size-3" />}
                    </span>
                    <span className="min-w-0 flex-1 truncate font-mono text-[11px]">{model.label}</span>
                    {picked && <Badge variant="primary">{index + 1}</Badge>}
                  </button>
                );
              })}
            </div>
          );
        })}
        {!catalog.length && (
          <p className="px-3 py-6 text-center font-mono text-[10px] text-muted-foreground/70">
            No models exposed yet.
          </p>
        )}
      </div>
      {selected.length > 0 && (
        <div className="flex flex-wrap gap-1.5 border border-border bg-secondary/40 p-2">
          {selected.map((id, index) => (
            <button
              key={id}
              onClick={() => toggle(id)}
              title="Click to remove"
              className="flex cursor-pointer items-center gap-1.5 border border-primary/30 bg-primary/5 px-1.5 py-1 font-mono text-[10px] text-primary hover:border-destructive/40 hover:text-destructive"
            >
              <span className="opacity-70">{index + 1}</span>
              {id}
              <span aria-hidden="true">×</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
