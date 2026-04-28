// Storage
pub const VAULT_READ_PUBLIC:   &str = "cap:vault.read.public";
pub const VAULT_WRITE_PUBLIC:  &str = "cap:vault.write.public";
pub const VAULT_READ_PRIVATE:  &str = "cap:vault.read.private";
pub const VAULT_WRITE_PRIVATE: &str = "cap:vault.write.private";
pub const VAULT_READ_SECRET:   &str = "cap:vault.read.secret";
pub const VAULT_WRITE_SECRET:  &str = "cap:vault.write.secret";
pub const VAULT_UNSEAL:        &str = "cap:vault.unseal";
pub const VAULT_AUDIT_READ:    &str = "cap:vault.audit.read";
// Behavioural
pub const STDOUT_FILTER:  &str = "cap:stdout.filter";
pub const CLI_SUBCOMMAND: &str = "cap:cli.subcommand";
pub const NET_OUTBOUND:   &str = "cap:net.outbound";
pub const SUBPROCESS:     &str = "cap:subprocess";

use std::collections::BTreeSet;

#[derive(Debug, Default, Clone)]
pub struct CapSet(BTreeSet<String>);

impl CapSet {
    pub fn new() -> Self { Self::default() }

    pub fn grant(&mut self, cap: impl Into<String>) { self.0.insert(cap.into()); }

    pub fn holds(&self, cap: &str) -> bool { self.0.contains(cap) }

    pub fn require(&self, cap: &str, plugin: &str) -> crate::Result<()> {
        if self.holds(cap) {
            Ok(())
        } else {
            Err(crate::Error::CapabilityDenied { cap: cap.into(), plugin: plugin.into() })
        }
    }
}

impl From<Vec<String>> for CapSet {
    fn from(v: Vec<String>) -> Self { Self(v.into_iter().collect()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_constants_present() {
        assert_eq!(VAULT_READ_PUBLIC, "cap:vault.read.public");
        assert_eq!(STDOUT_FILTER, "cap:stdout.filter");
    }

    #[test]
    fn capset_membership() {
        let mut s = CapSet::default();
        s.grant(VAULT_READ_PUBLIC);
        assert!(s.holds(VAULT_READ_PUBLIC));
        assert!(!s.holds(VAULT_READ_SECRET));
    }

    #[test]
    fn capset_require_err_on_missing() {
        let s = CapSet::default();
        let err = s.require(VAULT_READ_SECRET, "demo").unwrap_err();
        assert!(matches!(err, crate::Error::CapabilityDenied { .. }));
    }
}
