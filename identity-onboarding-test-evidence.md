# Identity/onboarding test evidence

Executed on DEBSRV after syncing the complete worktree to `~/boru-build/t_acd95a50/src`:

```text
cd ~/boru-build/t_acd95a50/src/desktop && pnpm test
Test Files  12 passed (12)
Tests       42 passed (42)
Duration    3.83s
```

Added test: `desktop/src/startup.test.tsx` exercises explicit onboarding creation with a display name, checks no network/connect/join IPC is invoked, and asserts secret/private-key/seed phrase patterns do not appear in the rendered DOM or captured console errors. Existing tests cover unchanged public endpoint ID when opening an existing profile, no implicit creation for no-identity startup, safe IPC error rendering, and privacy copy.

Coverage gap observed in this checkout: no React UI/components or command wiring for join-by-ticket or alias setup were found under `desktop/src`; therefore those requested screens cannot be meaningfully E2E-tested against this implementation. The test suite does not claim those paths are covered. The offline test uses mocked IPC; it demonstrates UI flow does not request network IPC, not real Rust-side offline identity persistence.
