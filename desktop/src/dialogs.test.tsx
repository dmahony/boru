import React from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import userEvent from "@testing-library/user-event";
import { ConfirmationDialog, SecurityApprovalDialog } from "./dialogs";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./components/ui";

afterEach(() => cleanup());

function ConfirmHarness({ security = false }: { security?: boolean }) {
  const [open, setOpen] = React.useState(false);
  return <><button onClick={() => setOpen(true)}>Open prompt</button>{security
    ? <SecurityApprovalDialog open={open} onOpenChange={setOpen} title="Approve device" description="Check the device fingerprint." />
    : <ConfirmationDialog open={open} onOpenChange={setOpen} title="Delete item?" description="This cannot be undone." confirmLabel="Delete" />}</>;
}

describe("dialog and overlay accessibility", () => {
  it("traps keyboard focus in a confirmation and closes with Escape", async () => {
    const user = userEvent.setup(); render(<ConfirmHarness />);
    const trigger = screen.getByRole("button", { name: "Open prompt" }); trigger.focus(); fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Delete item?" });
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(dialog.contains(document.activeElement)).toBe(true);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("does not allow Escape to dismiss a security approval", async () => {
    const user = userEvent.setup(); render(<ConfirmHarness security />);
    fireEvent.click(screen.getByRole("button", { name: "Open prompt" }));
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog", { name: "Approve device" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("renders menu content in a portal with collision-aware placement", async () => {
    const user = userEvent.setup();
    render(<DropdownMenu><DropdownMenuTrigger asChild><button>Actions</button></DropdownMenuTrigger><DropdownMenuContent side="bottom" align="end"><DropdownMenuItem>Delete</DropdownMenuItem></DropdownMenuContent></DropdownMenu>);
    await user.click(screen.getByRole("button", { name: "Actions" }));
    const menu = await screen.findByRole("menu");
    expect(menu.parentElement?.getAttribute("data-radix-popper-content-wrapper")).not.toBeNull();
  });
});
