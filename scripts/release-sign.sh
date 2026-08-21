#!/usr/bin/env bash
# Optional, credential-safe release signing hooks.
# No credentials are stored in this repository. An explicitly configured hook
# must succeed; an entirely unconfigured hook is reported as intentionally skipped.
set -euo pipefail

mode=${1:?usage: release-sign.sh <linux|windows|macos> <path>}
path=${2:?usage: release-sign.sh <linux|windows|macos> <path>}
case "$mode" in
  linux)
    if [[ -z "${BORU_LINUX_SIGNING_KEY:-}" ]]; then
      echo "Linux manifest signing skipped: BORU_LINUX_SIGNING_KEY is not configured"
      exit 0
    fi
    command -v openssl >/dev/null || { echo "openssl is required for configured Linux signing" >&2; exit 1; }
    openssl dgst -sha256 -sign "$BORU_LINUX_SIGNING_KEY" -out "$path.sig" "$path"
    openssl dgst -sha256 -verify "${BORU_LINUX_SIGNING_PUBLIC_KEY:?BORU_LINUX_SIGNING_PUBLIC_KEY is required}" -signature "$path.sig" "$path"
    ;;
  windows)
    if [[ -z "${BORU_WINDOWS_SIGNTOOL:-}" ]]; then
      echo "Windows Authenticode signing skipped: BORU_WINDOWS_SIGNTOOL is not configured"
      exit 0
    fi
    [[ -n "${BORU_WINDOWS_CERTIFICATE:-}" && -n "${BORU_WINDOWS_TIMESTAMP_URL:-}" ]] || { echo "configured Windows signing requires BORU_WINDOWS_CERTIFICATE and BORU_WINDOWS_TIMESTAMP_URL" >&2; exit 1; }
    "$BORU_WINDOWS_SIGNTOOL" sign /fd SHA256 /tr "$BORU_WINDOWS_TIMESTAMP_URL" /f "$BORU_WINDOWS_CERTIFICATE" "$path"
    "$BORU_WINDOWS_SIGNTOOL" verify /pa /all "$path"
    ;;
  macos)
    if [[ -z "${BORU_MACOS_SIGN_IDENTITY:-}" ]]; then
      echo "macOS signing/notarization skipped: BORU_MACOS_SIGN_IDENTITY is not configured"
      exit 0
    fi
    command -v codesign >/dev/null || { echo "codesign is required for configured macOS signing" >&2; exit 1; }
    codesign --force --timestamp --options runtime --sign "$BORU_MACOS_SIGN_IDENTITY" "$path"
    codesign --verify --deep --strict --verbose=2 "$path"
    if [[ -n "${BORU_MACOS_NOTARY_PROFILE:-}" ]]; then
      command -v xcrun >/dev/null || { echo "xcrun is required for configured macOS notarization" >&2; exit 1; }
      xcrun notarytool submit "$path" --keychain-profile "$BORU_MACOS_NOTARY_PROFILE" --wait
      xcrun stapler staple "$path"
      xcrun stapler validate "$path"
    fi
    ;;
  *) echo "unknown signing mode: $mode" >&2; exit 2 ;;
esac
