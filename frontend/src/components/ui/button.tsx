import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-[11px] font-semibold transition-[color,background,border-color,transform] active:translate-y-px disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-3.5 [&_svg]:shrink-0 outline-none focus-visible:ring-2 focus-visible:ring-ring/60 cursor-pointer",
  {
    variants: {
      variant: {
        default:
          "border border-primary bg-primary text-primary-foreground hover:bg-primary/90 hover:border-primary",
        secondary:
          "border border-input bg-secondary text-foreground hover:bg-accent",
        ghost:
          "min-h-7 border-transparent bg-transparent px-2.5 text-muted-foreground hover:text-foreground hover:border-border",
        destructive:
          "min-h-7 border-transparent bg-transparent px-2.5 text-destructive hover:border-destructive/25 hover:bg-destructive/10",
        outline:
          "border border-input bg-transparent text-foreground hover:bg-accent",
      },
      size: {
        default: "min-h-9 px-3.5",
        sm: "min-h-7 px-2.5",
        icon: "size-9",
      },
    },
    defaultVariants: { variant: "secondary", size: "default" },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {}

function Button({ className, variant, size, ...props }: ButtonProps) {
  return <button className={cn(buttonVariants({ variant, size }), className)} {...props} />;
}

export { Button, buttonVariants };
