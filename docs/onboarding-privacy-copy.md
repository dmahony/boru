# Boru onboarding privacy copy — canonical

Status: approved by the product owner on 2026-11-27 as the canonical, non-provisional shipped copy.
This supersedes the earlier `PROVISIONAL — pending owner review` header.

This document is informational copy, not a legal policy. Each statement is rendered by the React onboarding screen through the corresponding i18n key: `onboarding.privacy.headline` and `onboarding.privacy.identity` in `IdentityGate` (`desktop/src/main.tsx`), and `onboarding.privacy.intro`, `onboarding.privacy.network`, `onboarding.privacy.storage`, `onboarding.privacy.relay` in `PrivacyInformation` (`desktop/src/privacy-onboarding.tsx`). `onboarding.privacy.title` and `onboarding.privacy.continue` are UI chrome, not claims.

- “Boru connects peers directly or through relay servers; it does not use a central chat server.” (`onboarding.privacy.network`) — Source: `README.md` describes Boru as peer-to-peer without a central server and lists direct discovery and relay paths.
- “Private, peer-to-peer messaging. Your identity stays on this device.” (`onboarding.privacy.headline`) — Source: the same peer-to-peer transport described in `README.md`; identity material is the profile-local `secret_key.txt` handled by `desktop/src-tauri/src/identity.rs`.
- “A new profile creates a local cryptographic identity. Your secret key is stored locally and is never shown in the app.” (`onboarding.privacy.identity`) — Source: `desktop/src-tauri/src/identity.rs` writes `secret_key.txt` inside the profile data directory and never returns the key over IPC; the onboarding screen only renders the endpoint id and display name.
- “Boru stores application data locally in SQLite.” (`onboarding.privacy.storage`) — Source: `AGENTS.md` identifies SQLite as the single source of truth for application data; `desktop/src-tauri/src/persistence.rs` opens the profile-local message database.
- “When a relay is used, it forwards encrypted transport traffic between peers.” (`onboarding.privacy.relay`) — Source: `ENCRYPTION.md` lines 12–16 describes Iroh QUIC TLS encryption and states that the relay forwards encrypted transport packets.

No additional claims about data access, retention, compliance, or guarantees are made.
