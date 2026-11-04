//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The version of the socket API's requests, responses and events, which the helper checks
/// before it asks anything. Bump it when one of them changes in a way an older client misreads.
pub const API: u32 = 1;

pub fn version() -> String {
    BASE_VERSION.to_string()
}

/// Orders builds, so an installer never puts an older one over a newer: build-mux.sh stamps its
/// UTC time, and a dev build is 0.
pub fn build() -> u64 {
    option_env!("SESH_BUILD").and_then(|stamp| stamp.parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {

    #[test]
    fn stable_version_defaults_to_cargo_version() {
        assert!(!super::version().is_empty());
    }
}
