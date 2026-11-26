import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Button, Input, Tabs, TabsList, TabsTrigger, TabsContent } from "./ui";

describe("Boru UI primitives", () => {
  it("renders tokenized button and labeled input", () => {
    render(
      <>
        <Button>Send</Button>
        <label htmlFor="message">Message</label>
        <Input id="message" />
      </>,
    );
    expect(screen.getByRole("button", { name: "Send" })).toBeTruthy();
    expect(screen.getByLabelText("Message")).toBeTruthy();
  });
  it("supports keyboard operation for tabs", async () => {
    const user = userEvent.setup();
    render(
      <Tabs defaultValue="one">
        <TabsList>
          <TabsTrigger value="one">One</TabsTrigger>
          <TabsTrigger value="two">Two</TabsTrigger>
        </TabsList>
        <TabsContent value="one">First panel</TabsContent>
        <TabsContent value="two">Second panel</TabsContent>
      </Tabs>,
    );
    const first = screen.getByRole("tab", { name: "One" });
    first.focus();
    await user.keyboard("{ArrowRight}");
    expect(
      screen.getByRole("tab", { name: "Two" }).getAttribute("data-state"),
    ).toBe("active");
    expect(screen.getByText("Second panel")).toBeTruthy();
  });
});
