import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export { StatusDot } from "@/layout/app-shell";

export function Eyebrow({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("eyebrow", className)}>{children}</div>;
}

/** The bordered metric card the Overview and Usage pages share. */
export function Metric({ label, value, detail, danger = false }: {
  label: string;
  value: ReactNode;
  detail?: string;
  danger?: boolean;
}) {
  return (
    <div className={cn("signal-card", danger && "metric-danger")}>
      <span className="eyebrow">{label}</span>
      <div>
        <strong className="metric-value">{value}</strong>
        {detail && <span className="metric-detail mt-2 block">{detail}</span>}
      </div>
    </div>
  );
}

export function PageIntro({ children }: { children: ReactNode }) {
  return <p className="mt-2 text-[12px] text-muted-foreground">{children}</p>;
}

export function SectionToolbar({ title, detail, action }: { title: string; detail: string; action?: ReactNode }) {
  return (
    <div className="flex items-end justify-between gap-6 max-[700px]:flex-col max-[700px]:items-start">
      <div>
        <div className="eyebrow">{title}</div>
        <p className="mt-2 text-[12px] text-muted-foreground">{detail}</p>
      </div>
      {action}
    </div>
  );
}

export function formatTokens(value: number) {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return String(value);
}

export function formatTime(value: string) {
  return value ? value.replace("T", " ").slice(0, 19) : "—";
}
