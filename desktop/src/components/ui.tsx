import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import * as TabsPrimitive from "@radix-ui/react-tabs";
import * as DropdownPrimitive from "@radix-ui/react-dropdown-menu";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import * as ScrollAreaPrimitive from "@radix-ui/react-scroll-area";
import { Toaster } from "sonner";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 rounded-[var(--radius-sm)] border px-4 py-2 text-sm font-medium transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 disabled:pointer-events-none disabled:opacity-50",
  {
    variants: {
      variant: {
        primary:
          "border-transparent bg-[var(--color-accent)] text-[var(--color-canvas)] hover:opacity-90",
        secondary:
          "border-[var(--color-border)] bg-[var(--color-surface)] hover:bg-[var(--color-surface-raised)]",
        destructive: "border-transparent bg-[var(--color-danger)] text-white",
        ghost:
          "border-transparent bg-transparent hover:bg-[var(--color-surface-raised)]",
      },
      size: { default: "min-h-10", sm: "min-h-8 px-3", icon: "size-10 p-2" },
    },
    defaultVariants: { variant: "primary", size: "default" },
  },
);
export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}
export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        ref={ref}
        className={cn(buttonVariants({ variant, size, className }))}
        {...props}
      />
    );
  },
);
Button.displayName = "Button";
export const Input = React.forwardRef<
  HTMLInputElement,
  React.InputHTMLAttributes<HTMLInputElement>
>(({ className, ...props }, ref) => (
  <input
    ref={ref}
    className={cn(
      "min-h-10 w-full rounded-[var(--radius-sm)] border border-[var(--color-border)] bg-[var(--color-surface)] px-3 text-[var(--color-text)] placeholder:text-[var(--color-text-muted)] focus-visible:outline-2 focus-visible:outline-[var(--color-focus)]",
      className,
    )}
    {...props}
  />
));
Input.displayName = "Input";
export const Dialog = DialogPrimitive.Root;
export const DialogTrigger = DialogPrimitive.Trigger;
export function DialogContent({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content>) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="fixed inset-0 z-[var(--z-overlay)] bg-black/60" />
      <DialogPrimitive.Content
        className={cn(
          "fixed left-1/2 top-1/2 z-[var(--z-dialog)] w-[min(90vw,32rem)] -translate-x-1/2 -translate-y-1/2 rounded-[var(--radius-lg)] border border-[var(--color-border)] bg-[var(--color-surface)] p-6 text-[var(--color-text)] shadow-[var(--shadow-dialog)] focus:outline-none",
          className,
        )}
        {...props}
      />
    </DialogPrimitive.Portal>
  );
}
export const DialogTitle = DialogPrimitive.Title;
export const DialogDescription = DialogPrimitive.Description;
export const Tabs = TabsPrimitive.Root;
export const TabsList = TabsPrimitive.List;
export const TabsTrigger = TabsPrimitive.Trigger;
export const TabsContent = TabsPrimitive.Content;
export const DropdownMenu = DropdownPrimitive.Root;
export const DropdownMenuTrigger = DropdownPrimitive.Trigger;
export const DropdownMenuContent = DropdownPrimitive.Content;
export const DropdownMenuItem = DropdownPrimitive.Item;
export const TooltipProvider = TooltipPrimitive.Provider;
export const Tooltip = TooltipPrimitive.Root;
export const TooltipTrigger = TooltipPrimitive.Trigger;
export const TooltipContent = TooltipPrimitive.Content;
export const ScrollArea = ScrollAreaPrimitive.Root;
export const ScrollBar = ScrollAreaPrimitive.Scrollbar;
export { Toaster };
