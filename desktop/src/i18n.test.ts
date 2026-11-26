import { describe, expect, it } from "vitest";
import { localeDirection, t } from "./i18n";

describe("desktop locale dictionary", () => {
  it("reuses the Iced English/French keys and interpolation", () => {
    expect(t("chat.composer.send")).toBe("Send");
    expect(t("chat.header.members", { count: 3 })).toBe("3 members");
  });
  it("falls back to English and keeps layout direction explicit", () => {
    expect(t("sidebar.chats")).toBe("Chats");
    expect(localeDirection("fr")).toBe("ltr");
    expect(t("not.a.real.translation.key")).toBe("not.a.real.translation.key");
  });
});
