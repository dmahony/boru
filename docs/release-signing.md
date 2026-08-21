# Release integrity hooks

The release workflow is fail-closed for validation, packaging, checksums, and any
signing operation that is explicitly configured. It never stores signing keys,
certificates, notarization profiles, or passwords in this repository.

## Optional GitHub configuration

Configure these repository/environment variables and secrets only in the GitHub
Actions environment that publishes releases:

- `BORU_WINDOWS_SIGNTOOL` (variable), `BORU_WINDOWS_TIMESTAMP_URL` (variable),
  and `BORU_WINDOWS_CERTIFICATE` (secret): Authenticode signing and `/pa` verification.
- `BORU_MACOS_SIGN_IDENTITY` (variable) and `BORU_MACOS_NOTARY_PROFILE` (secret
  or runner keychain profile): `codesign`, `notarytool --wait`, and `stapler`.
- `BORU_LINUX_SIGNING_KEY` and `BORU_LINUX_SIGNING_PUBLIC_KEY` (secrets):
  OpenSSL SHA-256 signature and immediate verification of `SHA256SUMS`.

The checked-in `scripts/release-sign.sh` is the common hook. When a complete
configuration is absent, it prints an explicit skip reason and succeeds because
unsigned publication is the documented default. If a signing mode is configured
but required companion values, tools, or verification fail, the hook exits
non-zero and the release cannot publish. No private key or certificate filename
is a credential; the secret values themselves must remain in GitHub Actions.

macOS signing expects an app bundle or other signable path supplied by a future
macOS packaging step. The current macOS artifact is a tarball, so the macOS hook
is not invoked against that archive; this is intentional rather than a claim that
a tarball is signed. The hook is ready for the packaged `.app` path and includes
notarization/stapling when `BORU_MACOS_NOTARY_PROFILE` is configured.

Every release payload, including the SPDX SBOM, is copied into one directory,
hashed into `SHA256SUMS`, and verified with `sha256sum --strict --check` before
GitHub Release publication. GitHub artifact attestations are also requested for
that same payload set.
