use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    fn parse(value: &str) -> Option<Self> {
        let value = value.strip_prefix('v').unwrap_or(value);
        let mut parts = value.split('.');
        Some(Self {
            major: parts.next()?.parse().ok()?,
            minor: parts.next()?.parse().ok()?,
            patch: parts.next()?.parse().ok()?,
        })
    }

    fn bump_major(self) -> Self {
        Self {
            major: self.major + 1,
            minor: 0,
            patch: 0,
        }
    }

    fn bump_minor(self) -> Self {
        Self {
            major: self.major,
            minor: self.minor + 1,
            patch: 0,
        }
    }

    fn bump_patch(self) -> Self {
        Self {
            major: self.major,
            minor: self.minor,
            patch: self.patch + 1,
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn command_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}

fn calculated_version(package_version: &str) -> String {
    let base = Version::parse(package_version).unwrap_or(Version {
        major: 0,
        minor: 0,
        patch: 0,
    });
    let Some(tags) = command_output(&["tag", "--list", "v[0-9]*", "--sort=-version:refname"])
    else {
        return base.to_string();
    };
    let Some(tag) = tags.lines().next() else {
        return base.to_string();
    };
    let Some(mut version) = Version::parse(&tag) else {
        return base.to_string();
    };
    let Some(commits) = command_output(&["log", &format!("{tag}..HEAD"), "--format=%B%x00"]) else {
        return version.to_string();
    };
    let mut has_breaking = false;
    let mut has_feature = false;
    let mut has_patch = false;
    for commit in commits.split('\0') {
        let subject = commit.lines().next().unwrap_or_default().trim();
        let is_breaking = subject.contains("!:")
            || commit.lines().any(|line| {
                line.trim_start().starts_with("BREAKING CHANGE:")
                    || line.trim_start().starts_with("BREAKING-CHANGE:")
            });
        let kind = subject.split(['(', ':', '!']).next().unwrap_or_default();
        if is_breaking {
            has_breaking = true;
        } else if kind == "feat" {
            has_feature = true;
        } else if matches!(kind, "fix" | "perf" | "refactor" | "revert") {
            has_patch = true;
        }
    }
    version = if has_breaking {
        version.bump_major()
    } else if has_feature {
        version.bump_minor()
    } else if has_patch {
        version.bump_patch()
    } else {
        version
    };
    version.to_string()
}

fn main() {
    // Capture git commit hash at build time
    let output = std::process::Command::new("git")
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
        .map(|s| s.trim().to_string());

    if let Some(hash) = output {
        if !hash.is_empty() {
            println!("cargo:rustc-env=GIT_HASH={}", hash);
        }
    }
    let package_version =
        std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    println!(
        "cargo:rustc-env=BORU_APP_VERSION={}",
        calculated_version(&package_version)
    );
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
    println!("cargo:rerun-if-changed=.git/refs/tags/");
}

#[cfg(test)]
mod tests {
    use super::Version;

    #[test]
    fn parses_and_formats_versions() {
        assert_eq!(Version::parse("v0.101.0").unwrap().to_string(), "0.101.0");
    }

    #[test]
    fn bump_resets_lower_components() {
        let version = Version {
            major: 1,
            minor: 2,
            patch: 3,
        };
        assert_eq!(version.bump_patch().to_string(), "1.2.4");
        assert_eq!(version.bump_minor().to_string(), "1.3.0");
        assert_eq!(version.bump_major().to_string(), "2.0.0");
    }
}
