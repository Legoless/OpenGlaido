//! API keys live in the OS keychain (macOS Keychain / Windows Credential Manager).
//! Model keys have one account per role/provider/endpoint; search keys are scoped by provider.

use crate::transcribe::TranscriptionConfig;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const SERVICE: &str = "com.openglaido.app";

fn entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| keychain_error("open", e))
}

fn keychain_error(action: &str, error: keyring::Error) -> String {
    format!("Couldn't {action} the keychain: {error}")
}

pub fn read(account: &str) -> Result<Option<String>, String> {
    match entry(account)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(keychain_error("read", e)),
    }
}

fn write(account: &str, value: Option<&str>) -> Result<(), String> {
    let entry = entry(account)?;
    let result = match value {
        Some(value) => entry.set_password(value),
        None => match entry.delete_credential() {
            Err(keyring::Error::NoEntry) => Ok(()),
            other => other,
        },
    };
    result.map_err(|e| keychain_error("update", e))
}

/// Stores a legacy key; an empty value deletes the entry.
pub fn set(account: &str, value: &str) -> Result<(), String> {
    write(account, (!value.is_empty()).then_some(value))
}

fn account(config: &TranscriptionConfig, role: &str) -> String {
    let (provider, endpoint) = if role == "stt" {
        (config.stt_provider.as_str(), config.endpoint_url.as_str())
    } else {
        (config.llm_provider.as_str(), config.llm_endpoint_url.as_deref().unwrap_or_default())
    };
    let provider = match provider {
        "groq" | "openai" | "elevenlabs" | "openrouter" | "mistral" | "gemini" | "ollama" | "lmstudio" => provider,
        _ => "custom",
    };
    let base = match reqwest::Url::parse(endpoint.trim()) {
        Ok(mut url) if matches!(url.scheme(), "http" | "https") && url.has_host() => {
            let path = url.path().trim_end_matches('/');
            let path = ["/audio/transcriptions", "/chat/completions", "/speech-to-text"]
                .iter().find_map(|suffix| path.strip_suffix(suffix)).unwrap_or(path).to_string();
            url.set_path(&path);
            url.to_string()
        }
        // Invalid/unfinished URLs cannot restore a key belonging to a valid endpoint.
        _ => format!("invalid:{}", endpoint.trim()),
    };
    let scope = hex::encode(Sha256::digest(format!("{provider}\0{base}")));
    format!("model_key_v1_{role}_{scope}")
}

fn other_role(account: &str, role: &str) -> String {
    account.replacen(&format!("_{role}_"), if role == "stt" { "_llm_" } else { "_stt_" }, 1)
}

fn decode(value: Option<String>) -> Result<Option<String>, String> {
    value.map(|value| value.strip_prefix("1:").map(str::to_string)
        .ok_or_else(|| "The saved provider key has an unsupported format".to_string())).transpose()
}

// The prefix gives a cleared key a nonempty stored value on every supported OS.
fn encode(value: &str) -> String {
    format!("1:{value}")
}

type ReadKey = dyn Fn(&str) -> Result<Option<String>, String>;
type WriteKey = dyn Fn(&str, Option<&str>) -> Result<(), String>;

/// Rolls back keychain writes unless config.json was successfully saved and commit() called.
pub struct ModelKeyUpdate {
    previous: Vec<(String, Option<String>)>,
    write: Box<WriteKey>,
}

impl ModelKeyUpdate {
    pub fn commit(mut self) {
        self.previous.clear();
    }

    fn rollback(&mut self) -> Result<(), String> {
        let mut failed = Vec::new();
        let mut errors = Vec::new();
        for (account, value) in self.previous.drain(..).rev() {
            if let Err(e) = (self.write)(&account, value.as_deref()) {
                errors.push(e);
                failed.push((account, value));
            }
        }
        self.previous = failed.into_iter().rev().collect();
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }
}

impl Drop for ModelKeyUpdate {
    fn drop(&mut self) {
        if let Err(e) = self.rollback() {
            eprintln!("Couldn't restore previous provider keys: {e}");
        }
    }
}

fn apply(
    updates: BTreeMap<String, String>,
    read: &ReadKey,
    write: impl Fn(&str, Option<&str>) -> Result<(), String> + 'static,
) -> Result<ModelKeyUpdate, String> {
    // Read all original values before changing anything, including on a locked keychain.
    let changes = updates.into_iter().map(|(account, value)| {
        let previous = read(&account)?;
        Ok((account, value, previous))
    }).collect::<Result<Vec<_>, String>>()?;
    let mut transaction = ModelKeyUpdate { previous: Vec::new(), write: Box::new(write) };
    for (account, value, previous) in changes {
        if previous.as_deref() == Some(value.as_str()) { continue; }
        // Record before writing, since a backend error need not guarantee no mutation occurred.
        transaction.previous.push((account.clone(), previous));
        if let Err(e) = (transaction.write)(&account, Some(&value)) {
            return Err(match transaction.rollback() {
                Ok(()) => e,
                Err(rollback) => format!("{e}; couldn't restore previous keys: {rollback}"),
            });
        }
    }
    Ok(transaction)
}

fn lookup(account: &str, updates: &BTreeMap<String, String>, read: &ReadKey) -> Result<Option<String>, String> {
    decode(match updates.get(account) { Some(value) => Some(value.clone()), None => read(account)? })
}

/// On first load, callers have already hydrated the two legacy keychain accounts.
pub fn load_model_keys(config: &mut TranscriptionConfig) -> Result<bool, String> {
    load_model_keys_with(config, &read, write)
}

fn load_model_keys_with(
    config: &mut TranscriptionConfig,
    read: &ReadKey,
    write: impl Fn(&str, Option<&str>) -> Result<(), String> + 'static,
) -> Result<bool, String> {
    if !config.provider_keys_migrated {
        let mut updates = BTreeMap::new();
        let mut resolved = Vec::new();
        for (role, legacy) in [("stt", &config.api_key), ("llm", &config.llm_api_key)] {
            let target = account(config, role);
            let key = match decode(read(&target)?)? {
                Some(key) => key, // A prior migration may have saved keys before config.json failed.
                None => {
                    updates.insert(target, encode(legacy));
                    legacy.clone()
                }
            };
            resolved.push(key);
        }
        apply(updates, read, write)?.commit();
        config.api_key = resolved.remove(0);
        config.llm_api_key = resolved.remove(0);
        config.provider_keys_migrated = true;
        return Ok(true);
    }
    let resolve = |role: &str| -> Result<String, String> {
        let account = account(config, role);
        match decode(read(&account)?)? {
            Some(key) => Ok(key),
            None => Ok(decode(read(&other_role(&account, role))?)?.unwrap_or_default()),
        }
    };
    // Resolve both first: a read failure must not half-change the runtime config.
    let stt = resolve("stt")?;
    let llm = resolve("llm")?;
    config.api_key = stt;
    config.llm_api_key = llm;
    Ok(false)
}

pub fn save_model_keys(old: &TranscriptionConfig, new: &mut TranscriptionConfig, edited_model_keys: &[&str]) -> Result<ModelKeyUpdate, String> {
    save_model_keys_with(old, new, edited_model_keys, &read, write)
}

fn save_model_keys_with(
    old: &TranscriptionConfig,
    new: &mut TranscriptionConfig,
    edited_model_keys: &[&str],
    read: &ReadKey,
    write: impl Fn(&str, Option<&str>) -> Result<(), String> + 'static,
) -> Result<ModelKeyUpdate, String> {
    let roles = [("stt", &old.api_key, &new.api_key), ("llm", &old.llm_api_key, &new.llm_api_key)];
    let mut updates = BTreeMap::new();
    for (role, old_key, new_key) in roles {
        let old_account = account(old, role);
        if (!old.provider_keys_migrated || !old_key.is_empty()) && read(&old_account)?.is_none() {
            updates.insert(old_account.clone(), encode(old_key));
        }
        // A scope switch carries the old key in the UI snapshot: never save it under the new scope.
        let key_field = if role == "stt" { "api_key" } else { "llm_api_key" };
        if old_account == account(new, role) && edited_model_keys.contains(&key_field) {
            updates.insert(old_account, encode(new_key));
        }
    }
    let mut resolved = Vec::new();
    for (role, _, _) in roles {
        let target = account(new, role);
        let key = match lookup(&target, &updates, read)? {
            Some(key) => key, // Includes an explicit clear; never refill it from the other role.
            None => match lookup(&other_role(&target, role), &updates, read)? {
                Some(key) if !key.is_empty() => {
                    updates.insert(target, encode(&key));
                    key
                }
                _ => String::new(),
            },
        };
        resolved.push(key);
    }
    let transaction = apply(updates, read, write)?;
    new.api_key = resolved.remove(0);
    new.llm_api_key = resolved.remove(0);
    new.provider_keys_migrated = true;
    Ok(transaction)
}

fn search_account(provider: &str) -> String {
    // Unknown/disabled legacy keys are retained without ever being restored for a real provider.
    let scope = match provider { "brave" | "tavily" => provider, _ => "unassigned" };
    format!("search_key_v1_{scope}")
}

pub fn get_search_provider_key(provider: &str) -> Result<String, String> {
    if !matches!(provider, "brave" | "tavily") { return Ok(String::new()); }
    Ok(decode(read(&search_account(provider))?)?.unwrap_or_default())
}

/// The caller hydrates the legacy search slot only before this migration succeeds.
pub fn load_search_key(config: &mut TranscriptionConfig) -> Result<bool, String> {
    load_search_key_with(config, &read, write)
}

fn load_search_key_with(
    config: &mut TranscriptionConfig,
    read: &ReadKey,
    write: impl Fn(&str, Option<&str>) -> Result<(), String> + 'static,
) -> Result<bool, String> {
    let migrated = !config.search_keys_migrated;
    let account = search_account(&config.search_provider);
    let stored = decode(read(&account)?)?;
    let mut updates = BTreeMap::new();
    let key = match stored {
        Some(key) => key,
        None if migrated => {
            updates.insert(account, encode(&config.search_api_key));
            config.search_api_key.clone()
        }
        None => String::new(),
    };
    apply(updates, read, write)?.commit();
    config.search_api_key = if matches!(config.search_provider.as_str(), "brave" | "tavily") { key } else { String::new() };
    config.search_keys_migrated = true;
    Ok(migrated)
}

pub fn save_search_key(old: &TranscriptionConfig, new: &mut TranscriptionConfig, edited_search_provider: Option<&str>) -> Result<ModelKeyUpdate, String> {
    save_search_key_with(old, new, edited_search_provider, &read, write)
}

fn save_search_key_with(
    old: &TranscriptionConfig,
    new: &mut TranscriptionConfig,
    edited_search_provider: Option<&str>,
    read: &ReadKey,
    write: impl Fn(&str, Option<&str>) -> Result<(), String> + 'static,
) -> Result<ModelKeyUpdate, String> {
    let mut updates = BTreeMap::new();
    let old_account = search_account(&old.search_provider);
    if (!old.search_keys_migrated || !old.search_api_key.is_empty()) && read(&old_account)?.is_none() {
        updates.insert(old_account, encode(&old.search_api_key));
    }
    let target = search_account(&new.search_provider);
    let known = matches!(new.search_provider.as_str(), "brave" | "tavily");
    if known && edited_search_provider == Some(new.search_provider.as_str()) {
        updates.insert(target.clone(), encode(&new.search_api_key));
    }
    let key = if known { lookup(&target, &updates, read)?.unwrap_or_default() } else { String::new() };
    let transaction = apply(updates, read, write)?;
    new.search_api_key = key;
    new.search_keys_migrated = true;
    Ok(transaction)
}

#[cfg(test)]
pub fn use_mock_store() {
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct Vault {
        values: Rc<RefCell<BTreeMap<String, String>>>,
        writes: Rc<Cell<usize>>,
        fail_write: Rc<Cell<Option<usize>>>,
        fail_read: Rc<RefCell<Option<String>>>,
    }

    impl Vault {
        fn reader(&self) -> impl Fn(&str) -> Result<Option<String>, String> + 'static {
            let vault = self.clone();
            move |account| {
                if vault.fail_read.borrow().as_deref() == Some(account) {
                    return Err("keychain locked".into());
                }
                Ok(vault.values.borrow().get(account).cloned())
            }
        }

        fn writer(&self) -> impl Fn(&str, Option<&str>) -> Result<(), String> + 'static {
            let vault = self.clone();
            move |account, value| {
                vault.writes.set(vault.writes.get() + 1);
                match value {
                    Some(value) => { vault.values.borrow_mut().insert(account.into(), value.into()); }
                    None => { vault.values.borrow_mut().remove(account); }
                }
                // Exercise the harder case: the backend reports failure after mutating storage.
                if vault.fail_write.get() == Some(vault.writes.get()) {
                    return Err("keychain write failed".into());
                }
                Ok(())
            }
        }

        fn load(&self, config: &mut TranscriptionConfig) -> Result<bool, String> {
            load_model_keys_with(config, &self.reader(), self.writer())
        }

        fn save(&self, old: &TranscriptionConfig, new: &mut TranscriptionConfig, edited: &[&str]) -> Result<ModelKeyUpdate, String> {
            save_model_keys_with(old, new, edited, &self.reader(), self.writer())
        }
    }

    fn legacy() -> TranscriptionConfig {
        TranscriptionConfig { api_key: "groq-stt".into(), llm_api_key: "groq-llm".into(), ..Default::default() }
    }

    fn openai(config: &mut TranscriptionConfig, role: &str) {
        if role == "stt" {
            config.stt_provider = "openai".into();
            config.endpoint_url = "https://api.openai.com/v1/audio/transcriptions".into();
        } else {
            config.llm_provider = "openai".into();
            config.llm_endpoint_url = Some("https://api.openai.com/v1/chat/completions".into());
        }
    }

    #[test]
    fn provider_switch_and_restart_restore_separate_legacy_keys() {
        let vault = Vault::default();
        let mut groq = legacy();
        assert_eq!(vault.load(&mut groq), Ok(true));
        let mut next = groq.clone();
        openai(&mut next, "stt");
        vault.save(&groq, &mut next, &[]).unwrap().commit();
        assert!(next.api_key.is_empty());
        let mut keyed = next.clone();
        keyed.api_key = "openai-stt".into();
        vault.save(&next, &mut keyed, &["api_key"]).unwrap().commit();
        let mut back = groq.clone();
        back.api_key.clear();
        vault.save(&keyed, &mut back, &[]).unwrap().commit();
        assert_eq!((&back.api_key, &back.llm_api_key), (&groq.api_key, &groq.llm_api_key));

        let mut restarted = keyed.clone();
        restarted.api_key.clear();
        restarted.llm_api_key.clear();
        assert_eq!(vault.load(&mut restarted), Ok(false));
        assert_eq!((restarted.api_key.as_str(), restarted.llm_api_key.as_str()), ("openai-stt", "groq-llm"));
        // Reload is read-only, and doesn't revisit generic legacy slots.
        let writes = vault.writes.get();
        vault.load(&mut restarted).unwrap();
        assert_eq!(vault.writes.get(), writes);
    }

    #[test]
    fn missing_role_copies_same_provider_once_but_explicit_clear_sticks() {
        let vault = Vault::default();
        let mut old = legacy();
        openai(&mut old, "stt");
        old.api_key = "openai-stt".into();
        vault.load(&mut old).unwrap();
        let mut shared = old.clone();
        openai(&mut shared, "llm");
        vault.save(&old, &mut shared, &[]).unwrap().commit();
        assert_eq!(shared.llm_api_key, "openai-stt");
        let mut edited = shared.clone();
        edited.llm_api_key = "separate-llm".into();
        vault.save(&shared, &mut edited, &["llm_api_key"]).unwrap().commit();
        assert_eq!(edited.api_key, "openai-stt");
        let mut cleared = edited.clone();
        cleared.llm_api_key.clear();
        vault.save(&edited, &mut cleared, &["llm_api_key"]).unwrap().commit();
        vault.load(&mut cleared).unwrap();
        assert!(cleared.llm_api_key.is_empty());
        let mut away = old.clone();
        vault.save(&cleared, &mut away, &[]).unwrap().commit();
        let mut back = shared.clone();
        vault.save(&away, &mut back, &[]).unwrap().commit();
        assert!(back.llm_api_key.is_empty());
        assert_eq!(back.api_key, "openai-stt");
    }

    #[test]
    fn scopes_bind_provider_and_actual_endpoint_with_safe_normalization() {
        let mut config = legacy();
        openai(&mut config, "stt");
        openai(&mut config, "llm");
        let original = account(&config, "stt");
        assert_eq!(other_role(&original, "stt"), account(&config, "llm"));
        config.endpoint_url = " https://API.OPENAI.COM:443/v1/audio/transcriptions/ ".into();
        assert_eq!(account(&config, "stt"), original);
        for url in [
            "https://evil.example/v1/audio/transcriptions",
            "http://api.openai.com/v1/audio/transcriptions",
            "https://api.openai.com:8443/v1/audio/transcriptions",
            "https://api.openai.com/other/audio/transcriptions",
            "https://api.openai.com/v1/audio/transcriptions?tenant=one",
            "https://api.openai.com/v1/audio/transcriptions?tenant=two",
            "not a url", "", "file:///v1/audio/transcriptions",
        ] {
            config.endpoint_url = url.into();
            assert_ne!(account(&config, "stt"), original, "{url}");
        }
        config.endpoint_url = "https://api.openai.com/v1/audio/transcriptions".into();
        config.stt_provider = "custom".into();
        let custom = account(&config, "stt");
        assert_ne!(custom, original);
        config.stt_provider = "unknown-custom-id".into();
        assert_eq!(account(&config, "stt"), custom);
        config.endpoint_url = "https://api.openai.com/other/audio/transcriptions".into();
        assert_ne!(account(&config, "stt"), custom);
    }

    #[test]
    fn custom_endpoint_switch_never_carries_a_key_to_another_host_or_base() {
        let vault = Vault::default();
        let mut first = legacy();
        first.stt_provider = "custom".into();
        first.endpoint_url = "https://example.com/team-a/audio/transcriptions?tenant=a".into();
        first.api_key = "custom-secret".into();
        vault.load(&mut first).unwrap();
        for endpoint in [
            "https://other.example.com/team-a/audio/transcriptions?tenant=a",
            "https://example.com/team-b/audio/transcriptions?tenant=a",
            "https://example.com/team-a/audio/transcriptions?tenant=b",
        ] {
            let mut next = first.clone();
            next.endpoint_url = endpoint.into();
            vault.save(&first, &mut next, &[]).unwrap().commit();
            assert!(next.api_key.is_empty());
            let mut back = first.clone();
            back.api_key.clear();
            vault.save(&next, &mut back, &[]).unwrap().commit();
            assert_eq!(back.api_key, "custom-secret");
        }
    }

    #[test]
    fn migration_failure_rolls_back_every_write_and_leaves_metadata_and_keys_intact() {
        let vault = Vault::default();
        let mut config = legacy();
        let original = config.clone();
        vault.fail_write.set(Some(2));
        assert!(vault.load(&mut config).is_err());
        assert_eq!(config, original);
        assert!(vault.values.borrow().is_empty());
        vault.fail_write.set(None);
        assert_eq!(vault.load(&mut config), Ok(true));
        assert_eq!(config.api_key, "groq-stt");
        assert_eq!(config.llm_api_key, "groq-llm");
    }

    #[test]
    fn failed_config_write_or_keychain_write_restores_existing_values() {
        let vault = Vault::default();
        let mut old = legacy();
        vault.load(&mut old).unwrap();
        let before = vault.values.borrow().clone();
        let mut next = old.clone();
        next.api_key = "new-stt".into();
        next.llm_api_key.clear();
        let update = vault.save(&old, &mut next, &["api_key", "llm_api_key"]).unwrap();
        assert_ne!(*vault.values.borrow(), before);
        drop(update); // config.json could not be written.
        assert_eq!(*vault.values.borrow(), before);
        vault.fail_write.set(Some(vault.writes.get() + 2));
        assert!(vault.save(&old, &mut next, &["api_key", "llm_api_key"]).is_err());
        assert_eq!(*vault.values.borrow(), before);
    }

    #[test]
    fn read_failure_never_overwrites_or_partially_hydrates_keys() {
        let vault = Vault::default();
        let mut old = legacy();
        vault.load(&mut old).unwrap();
        let before = vault.values.borrow().clone();
        *vault.fail_read.borrow_mut() = Some(account(&old, "stt"));
        let mut next = old.clone();
        next.api_key = "changed".into();
        let writes = vault.writes.get();
        assert!(vault.save(&old, &mut next, &["api_key", "llm_api_key"]).is_err());
        assert_eq!(vault.writes.get(), writes);
        assert_eq!(*vault.values.borrow(), before);
        let mut restarted = old.clone();
        restarted.api_key.clear();
        restarted.llm_api_key.clear();
        let original = restarted.clone();
        assert!(vault.load(&mut restarted).is_err());
        assert_eq!(restarted, original);
    }

    #[test]
    fn caller_cannot_undo_migration_and_model_changes_keep_keys() {
        let vault = Vault::default();
        let mut old = legacy();
        vault.load(&mut old).unwrap();
        let writes = vault.writes.get();
        let mut next = old.clone();
        next.provider_keys_migrated = false;
        next.model_name = "a-different-model".into();
        next.llm_source = "off".into();
        vault.save(&old, &mut next, &[]).unwrap().commit();
        assert!(next.provider_keys_migrated);
        assert_eq!(next.api_key, old.api_key);
        assert_eq!(next.llm_api_key, old.llm_api_key);
        assert_eq!(vault.writes.get(), writes);
    }

    #[test]
    fn failed_provider_switch_cannot_turn_a_queued_ui_reset_into_a_key_deletion() {
        let vault = Vault::default();
        let mut old = legacy();
        vault.load(&mut old).unwrap();
        // The backend still has Groq, but the UI queued switching back with its blank draft.
        let mut queued = old.clone();
        queued.api_key.clear();
        queued.llm_api_key.clear();
        vault.save(&old, &mut queued, &[]).unwrap().commit();
        assert_eq!(queued.api_key, old.api_key);
        assert_eq!(queued.llm_api_key, old.llm_api_key);
        let mut cleared = queued.clone();
        cleared.api_key.clear();
        vault.save(&queued, &mut cleared, &["api_key"]).unwrap().commit();
        vault.load(&mut cleared).unwrap();
        assert!(cleared.api_key.is_empty());
        assert_eq!(cleared.llm_api_key, old.llm_api_key);
    }

    #[test]
    fn retrying_migration_preserves_newer_scoped_keys_and_cleared_tombstones() {
        let vault = Vault::default();
        let mut migrated = legacy();
        vault.load(&mut migrated).unwrap();
        let mut edited = migrated.clone();
        edited.api_key = "newer-stt".into();
        edited.llm_api_key.clear();
        vault.save(&migrated, &mut edited, &["api_key", "llm_api_key"]).unwrap().commit();
        let before = vault.values.borrow().clone();
        let writes = vault.writes.get();

        let mut stale = legacy();
        assert_eq!(vault.load(&mut stale), Ok(true));
        assert_eq!(stale.api_key, "newer-stt");
        assert!(stale.llm_api_key.is_empty());
        assert_eq!(vault.writes.get(), writes);
        assert_eq!(*vault.values.borrow(), before);

        // Saving a stale config with no key edit must use the same existing scoped entries.
        let stale = legacy();
        let mut saved = stale.clone();
        vault.save(&stale, &mut saved, &[]).unwrap().commit();
        assert!(saved.provider_keys_migrated);
        assert_eq!(saved.api_key, "newer-stt");
        assert!(saved.llm_api_key.is_empty());
        assert_eq!(*vault.values.borrow(), before);
    }

    #[test]
    fn search_provider_keys_migrate_switch_clear_and_rollback() {
        let vault = Vault::default();
        let mut brave = TranscriptionConfig { search_provider: "brave".into(), search_api_key: "brave-secret".into(), ..Default::default() };
        assert!(load_search_key_with(&mut brave, &vault.reader(), vault.writer()).unwrap());
        let mut tavily = brave.clone();
        tavily.search_provider = "tavily".into();
        save_search_key_with(&brave, &mut tavily, None, &vault.reader(), vault.writer()).unwrap().commit();
        assert!(tavily.search_api_key.is_empty());
        let mut keyed = tavily.clone();
        keyed.search_api_key = "tavily-secret".into();
        save_search_key_with(&tavily, &mut keyed, Some("tavily"), &vault.reader(), vault.writer()).unwrap().commit();
        let mut back = keyed.clone();
        back.search_provider = "brave".into();
        save_search_key_with(&keyed, &mut back, None, &vault.reader(), vault.writer()).unwrap().commit();
        assert_eq!(back.search_api_key, "brave-secret");

        let before = vault.values.borrow().clone();
        let mut cleared = keyed.clone();
        cleared.search_api_key.clear();
        drop(save_search_key_with(&keyed, &mut cleared, Some("tavily"), &vault.reader(), vault.writer()).unwrap());
        assert_eq!(*vault.values.borrow(), before); // Failed config save keeps the old password.
        save_search_key_with(&keyed, &mut cleared, Some("tavily"), &vault.reader(), vault.writer()).unwrap().commit();
        cleared.search_keys_migrated = false;
        cleared.search_api_key = "stale-legacy-value".into();
        load_search_key_with(&mut cleared, &vault.reader(), vault.writer()).unwrap();
        assert!(cleared.search_api_key.is_empty()); // Tombstone wins over migration retries.
        assert_eq!(decode(vault.reader()(&search_account("brave")).unwrap()).unwrap(), Some("brave-secret".into()));
    }

    #[test]
    fn ambiguous_search_keys_are_never_reassigned_and_vault_failures_abort() {
        let vault = Vault::default();
        let mut unknown = TranscriptionConfig { search_api_key: "unknown-owner".into(), ..Default::default() };
        load_search_key_with(&mut unknown, &vault.reader(), vault.writer()).unwrap();
        assert!(unknown.search_api_key.is_empty());
        let mut brave = unknown.clone();
        brave.search_provider = "brave".into();
        save_search_key_with(&unknown, &mut brave, None, &vault.reader(), vault.writer()).unwrap().commit();
        assert!(brave.search_api_key.is_empty());
        let before = vault.values.borrow().clone();
        let mut keyed = brave.clone();
        keyed.search_api_key = "new-key".into();
        vault.fail_write.set(Some(vault.writes.get() + 1));
        assert!(save_search_key_with(&brave, &mut keyed, Some("brave"), &vault.reader(), vault.writer()).is_err());
        assert_eq!(*vault.values.borrow(), before);
        *vault.fail_read.borrow_mut() = Some(search_account("brave"));
        assert!(load_search_key_with(&mut brave, &vault.reader(), vault.writer()).is_err());
    }
}
