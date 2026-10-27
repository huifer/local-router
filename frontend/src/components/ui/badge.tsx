import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center gap-1 border px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-[0.06em] whitespace-nowrap",
  {
    variants: {
      variant: {
        default: "border-border bg-transparent text-muted-foreground",
        primary: "border-primary/30 bg-primary/5 text-primary",
        outline: "border-border text-foreground",
        destructive: "border-destructive/25 bg-destructive/5 text-destructive",
      },
    },
    defaultVariants: { variant: "default" },
  },
);

function Badge({ className, variant, ...props }: React.ComponentProps<"span"> & VariantProps<typeof badgeVariants>) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}

export { Badge, badgeVariants };
