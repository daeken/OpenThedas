//! A credential held in memory only.

use zeroize::Zeroizing;

/// A password, a sign-in code or a token. Never printed: `Debug` shows `[redacted]`, and
/// there is no `Display` or `Serialize`. The memory is zeroed when it is dropped.
#[derive(Clone, Default)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(Zeroizing::new(s.into()))
    }

    /// The value, to hand to the store. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Secret::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_printed() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s:?}"), "[redacted]");
        assert_eq!(s.expose(), "hunter2");
        assert!(Secret::new("  ").is_empty());
    }
}
