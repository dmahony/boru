import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import english from "../../src/bin/boru/locales/en.json";
import doc from "../../docs/onboarding-privacy-copy.md?raw";
import { PrivacyInformation } from "./privacy-onboarding";

const locale = english as Record<string, string>;
/** Every `onboarding.privacy.*` key the traceability doc claims is rendered. */
const documentedKeys = [...doc.matchAll(/`(onboarding\.privacy\.[a-z_.]+)`/g)].map(match => match[1] as string);

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

  it("renders only statements that the traceability doc documents", () => {
    expect(documentedKeys.length).toBeGreaterThan(0);
    for (const key of documentedKeys) {
      expect(locale[key], `documented key ${key} must exist in en.json`).toBeTruthy();
    }
    render(<PrivacyInformation onContinue={() => {}} />);
    const renderedKeys = ["onboarding.privacy.intro", "onboarding.privacy.network", "onboarding.privacy.storage", "onboarding.privacy.relay"];
    for (const key of renderedKeys) {
      expect(documentedKeys, `${key} is rendered but not documented`).toContain(key);
      expect(screen.getAllByText(locale[key]).length).toBeGreaterThan(0);
    }
  });
});
