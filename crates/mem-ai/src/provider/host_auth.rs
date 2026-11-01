//! `HostAuthorization` — the credential the OpenCode host requires.
//!
//! The live host accepts HTTP basic auth (`opencode` + the password recorded in
//! `~/.local/state/opencode/service.json`) on `{origin}/api`. The password is
//! held in a [`Zeroizing`] buffer from the moment it is parsed, every derived
//! copy is zeroizing too, and the only formatter this type has is a
//! **redacted** [`Debug`]. There is deliberately **no `Display`** impl: the
//! credential must never be printable, only sendable.
//!
//! Residuals (stated, not claimed away): the base64 header is materialised in a
//! `String` for the request, and `reqwest` copies it into a `HeaderValue` that
//! lives for the request.

use std::fmt;

pub use zeroize::Zeroizing;

use base64::Engine as _;

/// Basic-auth credential for the OpenCode host.
pub struct HostAuthorization {
    username: String,
    password: Zeroizing<String>,
}

impl HostAuthorization {
    /// Build the credential, **moving** the already-zeroizing secret.
    pub fn basic_from_owned(username: impl Into<String>, password: Zeroizing<String>) -> Self {
        Self {
            username: username.into(),
            password,
        }
    }

    /// The `Authorization` header value (`Basic <base64(user:pass)>`).
    ///
    /// Both intermediate buffers are zeroizing; the returned value is the only
    /// plaintext-adjacent copy the caller holds, and it is zeroized on drop.
    pub fn header_value(&self) -> Zeroizing<String> {
        let joined = Zeroizing::new(format!("{}:{}", self.username, *self.password));
        let encoded =
            Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(joined.as_bytes()));
        Zeroizing::new(format!("Basic {}", *encoded))
    }
}

// A manual `Debug` on purpose: a required redaction, not a derived one (the
// inner `Zeroizing`'s own `Debug` is redacted by the crate, so a derived
// formatter would pass a leak test while proving nothing about this impl).
impl fmt::Debug for HostAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HostAuthorization(<redacted>)")
    }
}

// A manual `Clone` on purpose: the derived one would also be zeroizing (the
// field type owns that), but writing it out keeps the rule visible where a
// plain `String` field could be introduced later. A second zeroizing copy.
impl Clone for HostAuthorization {
    fn clone(&self) -> Self {
        Self {
            username: self.username.clone(),
            password: Zeroizing::new(self.password.to_string()),
        }
    }
}

impl PartialEq for HostAuthorization {
    fn eq(&self, other: &Self) -> bool {
        self.username == other.username && *self.password == *other.password
    }
}

impl Eq for HostAuthorization {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_value_is_basic_over_user_and_password() {
        let authorization =
            HostAuthorization::basic_from_owned("opencode", Zeroizing::new("s3cret".to_string()));
        assert_eq!(
            authorization.header_value().as_str(),
            "Basic b3BlbmNvZGU6czNjcmV0"
        );
    }

    #[test]
    fn debug_is_redacted_and_clone_keeps_the_secret() {
        let authorization =
            HostAuthorization::basic_from_owned("opencode", Zeroizing::new("s3cret".to_string()));
        assert_eq!(
            format!("{authorization:?}"),
            "HostAuthorization(<redacted>)"
        );
        assert_eq!(authorization.clone(), authorization);
        assert_eq!(
            authorization.clone().header_value().as_str(),
            "Basic b3BlbmNvZGU6czNjcmV0"
        );
    }
}
