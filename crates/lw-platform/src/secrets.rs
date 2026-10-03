//! Secure secret storage (API keys) via the OS credential store, wrapped over `keyring`.
//!
//! Windows → Credential Manager, macOS → Keychain, Linux → Secret Service / keyutils.

use crate::{Error, Result};

/// A secret store scoped to one service name (e.g. `"OwlWhisp"`).
#[derive(Clone, Debug)]
pub struct SecureStore {
    service: String,
}

impl SecureStore {
    /// A store scoped to `service`.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    /// The service name this store is scoped to.
    pub fn service(&self) -> &str {
        &self.service
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key).map_err(|e| Error::Secrets(e.to_string()))
    }

    /// Store `secret` under `key` (overwrites).
    pub fn set(&self, key: &str, secret: &str) -> Result<()> {
        self.entry(key)?
            .set_password(secret)
            .map_err(|e| Error::Secrets(e.to_string()))
    }

    /// Fetch the secret stored under `key`, or `None` if absent.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        match self.entry(key)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Secrets(e.to_string())),
        }
    }

    /// Delete the secret stored under `key`. Deleting a missing key is not an error.
    pub fn delete(&self, key: &str) -> Result<()> {
        match self.entry(key)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Secrets(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_is_kept() {
        let store = SecureStore::new("OwlWhispTest");
        assert_eq!(store.service(), "OwlWhispTest");
    }

    /// Round-trip against the real OS credential store. Ignored by default so plain
    /// `cargo test` stays hermetic; run with `cargo test -- --ignored` on a machine
    /// where writing to the credential store is acceptable.
    #[test]
    #[ignore = "touches the OS credential store"]
    fn roundtrip_in_os_store() {
        let store = SecureStore::new("OwlWhispTest");
        store.set("test-key", "s3cret").unwrap();
        assert_eq!(store.get("test-key").unwrap().as_deref(), Some("s3cret"));
        store.delete("test-key").unwrap();
        assert_eq!(store.get("test-key").unwrap(), None);
        // double delete is fine
        store.delete("test-key").unwrap();
    }
}
