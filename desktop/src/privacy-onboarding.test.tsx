import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import english from "../../src/bin/boru/locales/en.json";
import doc from "../../docs/onboarding-privacy-copy.md?raw";
import { PrivacyInformation } from "./privacy-onboarding";
import { IdentityGate } from "./main";

const locale = english as Record<string, string>;
const documentedKeys = [...doc.matchAll(/`(onboarding\.privacy\.[a-z_.]+)`/g)].map(match => match[1] as string);
const claimKeys = ["onboarding.privacy.headline", "onboarding.privacy.identity", "onboarding.privacy.intro", "onboarding.privacy.network", "onboarding.privacy.storage", "onboarding.privacy.relay"];

describe("PrivacyInformation", () => {
  it("shows exact locale copy and advances only on continue", () => {
    const onContinue = vi.fn();
    render(<PrivacyInformation onContinue={onContinue} />);
    for (const key of ["onboarding.privacy.title", "onboarding.privacy.intro", "onboarding.privacy.network", "onboarding.privacy.storage", "onboarding.privacy.relay", "onboarding.privacy.continue"]) {
      expect(screen.getByText(locale[key])).toBeTruthy();
    }
    expect(onContinue).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: locale["onboarding.privacy.continue"] }));
    expect(onContinue).toHaveBeenCalledOnce();
  });

  it("matches every claim exactly to en.json and requires documentation", () => {
    expect(documentedKeys.length).toBeGreaterThan(0);
    for (const key of claimKeys) {
      expect(locale[key], `missing locale value for ${key}`).toBeTruthy();
      expect(documentedKeys, `${key} is rendered but not documented`).toContain(key);
    }
    render(<PrivacyInformation onContinue={() => {}} />);
    for (const key of ["onboarding.privacy.intro", "onboarding.privacy.network", "onboarding.privacy.storage", "onboarding.privacy.relay"]) {
      expect(screen.getAllByText(locale[key]).every(element => element.textContent === locale[key])).toBe(true);
    }
  });

  it("IdentityGate renders the exact identity locale value", async () => {
    vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async (command: string) => command === "get_startup_state" ? { state: "no_identity" } : []) }));
    render(<IdentityGate />);
    expect(await screen.findByText(locale["onboarding.privacy.identity"])).toHaveProperty("textContent", locale["onboarding.privacy.identity"]);
  });
});
