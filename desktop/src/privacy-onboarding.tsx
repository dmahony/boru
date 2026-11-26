import { t } from "./i18n";

export function PrivacyInformation({ onContinue }: { onContinue: () => void }) {
  return <main className="privacy-onboarding" aria-labelledby="privacy-onboarding-title">
    <section className="privacy-card">
      <h1 id="privacy-onboarding-title">{t("onboarding.privacy.title")}</h1>
      <p>{t("onboarding.privacy.intro")}</p>
      <ul>
        <li>{t("onboarding.privacy.network")}</li>
        <li>{t("onboarding.privacy.storage")}</li>
        <li>{t("onboarding.privacy.relay")}</li>
      </ul>
      <button onClick={onContinue}>{t("onboarding.privacy.continue")}</button>
    </section>
  </main>;
}
