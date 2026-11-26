# PROVISIONAL — pending owner review

This document is informational copy, not a legal policy. Each statement is rendered by the React onboarding screen through the corresponding i18n key.

- “Boru connects peers directly or through relay servers; it does not use a central chat server.” (`onboarding.privacy.network`) — Source: `README.md` describes Boru as peer-to-peer without a central server and lists direct discovery and relay paths.
- “A new profile creates a local cryptographic identity. Your secret key is stored locally and is never shown in the app.” (`onboarding.privacy.identity`) — Source: `desktop/src-tauri/src/identity.rs` writes `secret_key.txt` inside the profile data directory and never returns the key over IPC; the onboarding screen only renders the endpoint id and display name.
- “Boru stores application data locally in SQLite.” (`onboarding.privacy.storage`) — Source: `AGENTS.md` identifies SQLite as the single source of truth for application data; `desktop/src-tauri/src/persistence.rs` opens the profile-local message database.
- “When a relay is used, it forwards encrypted transport traffic between peers.” (`onboarding.privacy.relay`) — Source: `ENCRYPTION.md` lines 12–16 describes Iroh QUIC TLS encryption and states that the relay forwards encrypted transport packets.

No additional claims about data access, retention, compliance, or guarantees are made.
