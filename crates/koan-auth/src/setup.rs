//! The code that guards making the first admin.
//!
//! A server with no account makes its first admin from a form, so that one in
//! a container needs no shell to become usable. Whoever reaches the form first
//! would become the admin, and a server's database can be empty for worse
//! reasons than a new install: a backup that was not found, a volume that was
//! not mounted. So the form also asks for a code the server shows only where
//! its operator looks (its log, its console) while it has no account.

use subtle::ConstantTimeEq;

use crate::AuthError;

/// A one-time code: four groups of four characters a person can read off a
/// log and type, about 79 bits.
#[derive(Clone)]
pub struct SetupCode(String);

impl SetupCode {
    pub fn new() -> Result<Self, AuthError> {
        crate::secret::typeable(16, 4).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `typed` is this code, whatever its case, spaces and dashes.
    /// Compared in constant time.
    pub fn matches(&self, typed: &str) -> bool {
        let normal = |s: &str| {
            s.chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        let (want, got) = (normal(&self.0), normal(typed));
        want.len() == got.len() && bool::from(want.as_bytes().ct_eq(got.as_bytes()))
    }
}

impl std::fmt::Debug for SetupCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SetupCode(…)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_typed_back_however_it_is_spaced() {
        let code = SetupCode::new().unwrap();
        assert_eq!(code.as_str().len(), 19);
        assert!(code.matches(code.as_str()));
        assert!(code.matches(&code.as_str().to_uppercase().replace('-', " ")));
        assert!(!code.matches(""));
        assert!(!code.matches(&code.as_str()[..18]));
        assert!(!SetupCode::new().unwrap().matches(code.as_str()));
        assert_eq!(format!("{code:?}"), "SetupCode(…)");
    }
}
