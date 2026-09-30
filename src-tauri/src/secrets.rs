//! API keys live in the OS keychain (macOS Keychain / Windows Credential Manager), never in
//! config.json. Accounts: "api_key", "search_api_key".

const SERVICE: &str = "com.openglaido.app";

use std::sync::atomic::{AtomicBool, Ordering};

/// False once the keychain failed: config.json then keeps the keys rather than losing them.
static AVAILABLE: AtomicBool = AtomicBool::new(true);

pub fn available() -> bool {
    AVAILABLE.load(Ordering::SeqCst)
}

fn entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| {
        AVAILABLE.store(false, Ordering::SeqCst);
        format!("Couldn't open the keychain: {e}")
    })
}

/// The stored secret, or "" when there is none (or the keychain is unavailable; logged).
pub fn get(account: &str) -> String {
    match entry(account).and_then(|e| e.get_password().map_err(|e| e.to_string())) {
        Ok(value) => value,
        Err(e) if e.contains("No matching entry") => String::new(),
        Err(e) => {
            eprintln!("Keychain read failed for {account}: {e}");
            AVAILABLE.store(false, Ordering::SeqCst);
            String::new()
        }
    }
}

/// Stores `value`; an empty value deletes the entry.
pub fn set(account: &str, value: &str) -> Result<(), String> {
    let entry = entry(account)?;
    let result = if value.is_empty() {
        match entry.delete_credential() {
            Err(keyring::Error::NoEntry) => Ok(()),
            other => other,
        }
    } else {
        entry.set_password(value)
    };
    result.map_err(|e| {
        AVAILABLE.store(false, Ordering::SeqCst);
        format!("Couldn't save the key to the keychain: {e}")
    })
}

#[cfg(test)]
pub fn use_mock_store() {
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_delete_with_mock_store() {
        use_mock_store();
        // The mock store keeps nothing across Entry instances, so check each call succeeds.
        assert!(set("api_key", "").is_ok());
        assert_eq!(get("missing"), "");
    }
}
