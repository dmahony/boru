/// Build script for Boru application.
///
/// The authoritative application version comes from Cargo.toml
/// (CARGO_PKG_VERSION). Version bumps are performed manually via
/// `scripts/version.py apply`, which updates Cargo.toml directly.
///
/// At build time we only capture the git commit hash for display
/// alongside the package version.
fn main() {
    // The Iced desktop view tree uses more stack during the first Windows
    // renderer/layout pass than the MSVC PE default reserve (1 MiB).  The
    // resulting stack-probe failure is reported as 0xc00000fd and can be
    // mistaken for a renderer or surface failure.  Reserve 8 MiB for the
    // Boru GUI executable while leaving other targets unchanged.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bin=boru=/STACK:0x800000");
    }

    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(h) = hash {
        println!("cargo:rustc-env=GIT_HASH={h}");
    }

    // BORU_APP_VERSION equals the authoritative Cargo.toml version.
    // No dynamic tag-based calculation — version bumps go through
    // scripts/version.py which updates Cargo.toml.
    let pkg_ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    println!("cargo:rustc-env=BORU_APP_VERSION={pkg_ver}");

    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
}
