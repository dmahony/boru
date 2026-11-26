import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { PrivacyInformation } from "./privacy-onboarding";

describe("PrivacyInformation", () => {
  it("shows the privacy information and advances only on continue", () => {
    const onContinue = vi.fn();
    render(<PrivacyInformation onContinue={onContinue} />);
    expect(screen.getByRole("heading", { name: "Your privacy in Boru" })).toBeTruthy();
    expect(screen.getByText(/does not use a central chat server/)).toBeTruthy();
    expect(screen.getByText(/stores application data locally in SQLite/)).toBeTruthy();
    expect(screen.getByText(/forwards encrypted transport traffic/)).toBeTruthy();
    expect(onContinue).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onContinue).toHaveBeenCalledOnce();
  });
});
