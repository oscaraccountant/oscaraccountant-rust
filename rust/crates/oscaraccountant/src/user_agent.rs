//! The `User-Agent` header (`clients/spec/README.md`, section 3.9).

/// The version of this crate, equal to `clients/VERSION`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `<product>/<version> (<os>; <arch>; rust)`, for example
/// `oscaraccountant-rust/0.1.0 (linux; x86_64; rust)`.
///
/// The CLI calls it with `oscar-cli` and its own version.
pub fn user_agent(product: &str, version: &str) -> String {
    format!(
        "{product}/{version} ({}; {}; rust)",
        os_name(std::env::consts::OS),
        arch_name(std::env::consts::ARCH)
    )
}

/// The `User-Agent` of the SDK.
pub(crate) fn default_user_agent() -> String {
    user_agent("oscaraccountant-rust", VERSION)
}

fn os_name(os: &str) -> &'static str {
    match os {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => "windows",
        _ => "unknown",
    }
}

fn arch_name(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_platform() {
        assert_eq!(os_name("linux"), "linux");
        assert_eq!(os_name("macos"), "macos");
        assert_eq!(os_name("windows"), "windows");
        assert_eq!(os_name("freebsd"), "unknown");
        assert_eq!(arch_name("aarch64"), "arm64");
        assert_eq!(arch_name("x86_64"), "x86_64");
    }

    #[test]
    fn builds_the_header() {
        let value = default_user_agent();
        assert!(
            value.starts_with(&format!("oscaraccountant-rust/{VERSION} (")),
            "{value}"
        );
        assert!(value.ends_with("; rust)"), "{value}");
        assert!(user_agent("oscar-cli", "1.2.3").starts_with("oscar-cli/1.2.3 ("));
    }

    #[test]
    fn the_version_is_the_one_of_the_clients() {
        // clients/VERSION, when the crate lives in the source repository (not in the exported repository).
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../VERSION");
        if let Ok(version) = std::fs::read_to_string(path) {
            assert_eq!(version.trim(), VERSION);
        }
    }
}
