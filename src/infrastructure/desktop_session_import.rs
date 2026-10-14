use std::fs;
use std::path::{Path, PathBuf};

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, KeyIvInit};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::domain::model::{Credentials, SlackError};

pub fn import_desktop_session(
    slack_data_dir: &Path,
    scratch_dir: &Path,
) -> Result<Credentials, SlackError> {
    let team = read_active_team(slack_data_dir, scratch_dir)?;
    let cookie = read_session_cookie(slack_data_dir, scratch_dir)?;
    Ok(Credentials {
        token: team.token,
        cookie,
        workspace_url: team.url,
        team_name: team.name,
    })
}

struct ActiveTeam {
    token: String,
    url: String,
    name: String,
}

fn read_active_team(slack_data_dir: &Path, scratch_dir: &Path) -> Result<ActiveTeam, SlackError> {
    let source = slack_data_dir.join(LOCAL_STORAGE_DIR);
    let copy = scratch_dir.join(LOCAL_STORAGE_COPY);
    copy_dir_without_lock(&source, &copy)?;
    let mut db = rusty_leveldb::DB::open(&copy, rusty_leveldb::Options::default())
        .map_err(|error| SlackError(format!("cannot open Slack local storage: {error}")))?;
    let raw = db
        .get(LOCAL_CONFIG_KEY)
        .ok_or_else(|| SlackError(String::from("Slack local config not found")))?;
    let config: Value = serde_json::from_str(&decode_local_storage_value(&raw.to_vec())?)
        .map_err(|error| SlackError(format!("invalid Slack local config: {error}")))?;
    parse_active_team(&config)
}

fn parse_active_team(config: &Value) -> Result<ActiveTeam, SlackError> {
    let teams = config["teams"]
        .as_object()
        .ok_or_else(|| SlackError(String::from("no teams in Slack local config")))?;
    let team = config["lastActiveTeamId"]
        .as_str()
        .and_then(|id| teams.get(id))
        .or_else(|| teams.values().next())
        .ok_or_else(|| SlackError(String::from("no signed-in team found")))?;
    let field = |name: &str| {
        team[name]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| SlackError(format!("team is missing {name}")))
    };
    Ok(ActiveTeam {
        token: field("token")?,
        url: field("url")?,
        name: field("name")?,
    })
}

fn decode_local_storage_value(raw: &[u8]) -> Result<String, SlackError> {
    match raw.split_first() {
        Some((&LATIN1_MARKER, rest)) => Ok(rest.iter().map(|&byte| byte as char).collect()),
        Some((&UTF16_MARKER, rest)) => {
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16(&units).map_err(|error| SlackError(error.to_string()))
        }
        _ => Err(SlackError(String::from("unknown local storage encoding"))),
    }
}

fn read_session_cookie(slack_data_dir: &Path, scratch_dir: &Path) -> Result<String, SlackError> {
    let copy = scratch_dir.join(COOKIES_COPY);
    fs::copy(slack_data_dir.join(COOKIES_FILE), &copy)
        .map_err(|error| SlackError(format!("cannot copy Slack cookies: {error}")))?;
    let connection = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| SlackError(error.to_string()))?;
    let schema_version: i64 = connection
        .query_row(META_VERSION_QUERY, [], |row| row.get::<_, String>(0))
        .map_err(|error| SlackError(error.to_string()))?
        .parse()
        .map_err(|_| SlackError(String::from("invalid cookie schema version")))?;
    let encrypted: Vec<u8> = connection
        .query_row(SESSION_COOKIE_QUERY, [], |row| row.get(0))
        .map_err(|_| SlackError(String::from("Slack session cookie not found")))?;
    let plaintext = decrypt_cookie(&encrypted)?;
    let value = if schema_version >= HASHED_DOMAIN_SCHEMA_VERSION {
        plaintext
            .get(DOMAIN_HASH_LENGTH..)
            .unwrap_or_default()
            .to_vec()
    } else {
        plaintext
    };
    String::from_utf8(value).map_err(|_| SlackError(String::from("cookie is not valid UTF-8")))
}

fn decrypt_cookie(encrypted: &[u8]) -> Result<Vec<u8>, SlackError> {
    let (prefix, ciphertext) = encrypted
        .split_at_checked(ENCRYPTION_PREFIX_LENGTH)
        .ok_or_else(|| SlackError(String::from("unsupported cookie encryption")))?;
    let passwords = safe_storage::passwords(prefix)?;
    passwords
        .iter()
        .find_map(|password| decrypt_with_password(ciphertext, password))
        .ok_or_else(|| SlackError(String::from("cookie decryption failed")))
}

fn decrypt_with_password(ciphertext: &[u8], password: &[u8]) -> Option<Vec<u8>> {
    let mut key = [0u8; KEY_LENGTH];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, KEY_SALT, safe_storage::KEY_ITERATIONS, &mut key);
    let mut buffer = ciphertext.to_vec();
    let plaintext = Aes128CbcDecryptor::new(&key.into(), &INITIALIZATION_VECTOR.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .ok()?;
    Some(plaintext.to_vec())
}

#[cfg(target_os = "macos")]
mod safe_storage {
    use std::process::Command;

    use crate::domain::model::SlackError;

    pub const KEY_ITERATIONS: u32 = 1003;

    pub fn passwords(prefix: &[u8]) -> Result<Vec<Vec<u8>>, SlackError> {
        if prefix != b"v10" {
            return Err(SlackError(String::from("unsupported cookie encryption")));
        }
        let output = Command::new(SECURITY_TOOL)
            .args(["find-generic-password", "-w", "-s", SAFE_STORAGE_SERVICE])
            .output()
            .map_err(|error| SlackError(error.to_string()))?;
        if !output.status.success() {
            return Err(SlackError(String::from(
                "Keychain access to Slack Safe Storage was denied",
            )));
        }
        let password = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(vec![password.into_bytes()])
    }

    const SECURITY_TOOL: &str = "security";
    const SAFE_STORAGE_SERVICE: &str = "Slack Safe Storage";
}

// Chromium on Linux encrypts with a fixed key ("v10") when no keyring is
// available, and with a key from the Secret Service otherwise ("v11").
#[cfg(target_os = "linux")]
mod safe_storage {
    use std::collections::HashMap;
    use std::time::Duration;

    use dbus::Path;
    use dbus::arg::{RefArg, Variant};
    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    use dbus::blocking::{Connection, Proxy};

    use crate::domain::model::SlackError;

    pub const KEY_ITERATIONS: u32 = 1;

    pub fn passwords(prefix: &[u8]) -> Result<Vec<Vec<u8>>, SlackError> {
        match prefix {
            b"v10" => Ok(vec![FIXED_PASSWORD.to_vec()]),
            // Chromium falls back to an empty key when the keyring returns nothing.
            b"v11" => Ok(vec![keyring_password()?, Vec::new()]),
            _ => Err(SlackError(String::from("unsupported cookie encryption"))),
        }
    }

    fn keyring_password() -> Result<Vec<u8>, SlackError> {
        let to_error =
            |error: dbus::Error| SlackError(format!("cannot read Slack Safe Storage: {error}"));
        let connection = Connection::new_session().map_err(to_error)?;
        let service = connection.with_proxy(SECRETS_BUS, SECRETS_PATH, TIMEOUT);
        let (_, session): (Variant<Box<dyn RefArg>>, Path) = service
            .method_call(
                SERVICE_INTERFACE,
                "OpenSession",
                ("plain", Variant(Box::new(String::new()) as Box<dyn RefArg>)),
            )
            .map_err(to_error)?;
        let mut unlocked = Vec::new();
        let mut locked = Vec::new();
        for schema in SCHEMAS {
            let attributes = HashMap::from([("xdg:schema", schema)]);
            let (found, found_locked): (Vec<Path>, Vec<Path>) = service
                .method_call(SERVICE_INTERFACE, "SearchItems", (attributes,))
                .map_err(to_error)?;
            unlocked.extend(found);
            locked.extend(found_locked);
        }
        let item = unlocked
            .into_iter()
            .find(|item| is_slack_item(&connection.with_proxy(SECRETS_BUS, item, TIMEOUT)))
            .ok_or_else(|| {
                SlackError(String::from(if locked.is_empty() {
                    "Slack Safe Storage not found in keyring"
                } else {
                    "keyring is locked"
                }))
            })?;
        let (secret,): ((Path, Vec<u8>, Vec<u8>, String),) = connection
            .with_proxy(SECRETS_BUS, &item, TIMEOUT)
            .method_call(ITEM_INTERFACE, "GetSecret", (session,))
            .map_err(to_error)?;
        Ok(secret.2)
    }

    fn is_slack_item(item: &Proxy<'_, &Connection>) -> bool {
        let label: String = item.get(ITEM_INTERFACE, "Label").unwrap_or_default();
        let attributes: HashMap<String, String> =
            item.get(ITEM_INTERFACE, "Attributes").unwrap_or_default();
        label == SAFE_STORAGE_LABEL
            || attributes
                .get("application")
                .is_some_and(|application| application.eq_ignore_ascii_case(APPLICATION))
    }

    const FIXED_PASSWORD: &[u8] = b"peanuts";
    const SECRETS_BUS: &str = "org.freedesktop.secrets";
    const SECRETS_PATH: &str = "/org/freedesktop/secrets";
    const SERVICE_INTERFACE: &str = "org.freedesktop.Secret.Service";
    const ITEM_INTERFACE: &str = "org.freedesktop.Secret.Item";
    const SCHEMAS: [&str; 2] = [
        "chrome_libsecret_os_crypt_password_v2",
        "chrome_libsecret_os_crypt_password_v1",
    ];
    const SAFE_STORAGE_LABEL: &str = "Slack Safe Storage";
    const APPLICATION: &str = "slack";
    const TIMEOUT: Duration = Duration::from_secs(5);
}

fn copy_dir_without_lock(source: &Path, target: &PathBuf) -> Result<(), SlackError> {
    let to_error =
        |error: std::io::Error| SlackError(format!("cannot copy Slack local storage: {error}"));
    if target.exists() {
        fs::remove_dir_all(target).map_err(to_error)?;
    }
    fs::create_dir_all(target).map_err(to_error)?;
    for entry in fs::read_dir(source).map_err(to_error)? {
        let entry = entry.map_err(to_error)?;
        if entry.file_name() != LOCK_FILE {
            fs::copy(entry.path(), target.join(entry.file_name())).map_err(to_error)?;
        }
    }
    Ok(())
}

type Aes128CbcDecryptor = cbc::Decryptor<aes::Aes128>;

const LOCAL_STORAGE_DIR: &str = "Local Storage/leveldb";
const LOCAL_STORAGE_COPY: &str = "local-storage-copy";
const LOCK_FILE: &str = "LOCK";
const LOCAL_CONFIG_KEY: &[u8] = b"_https://app.slack.com\x00\x01localConfig_v2";
const LATIN1_MARKER: u8 = 1;
const UTF16_MARKER: u8 = 0;
const COOKIES_FILE: &str = "Cookies";
const COOKIES_COPY: &str = "cookies-copy.sqlite";
const META_VERSION_QUERY: &str = "SELECT value FROM meta WHERE key = 'version'";
const SESSION_COOKIE_QUERY: &str = "SELECT encrypted_value FROM cookies WHERE name = 'd' AND host_key LIKE '%slack.com' ORDER BY length(host_key) LIMIT 1";
const HASHED_DOMAIN_SCHEMA_VERSION: i64 = 24;
const DOMAIN_HASH_LENGTH: usize = 32;
const ENCRYPTION_PREFIX_LENGTH: usize = 3;
const KEY_LENGTH: usize = 16;
const KEY_SALT: &[u8] = b"saltysalt";
const INITIALIZATION_VECTOR: [u8; 16] = [b' '; 16];

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use aes::cipher::BlockEncryptMut;

    use super::*;

    fn encrypt(prefix: &[u8], password: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut key = [0u8; KEY_LENGTH];
        pbkdf2::pbkdf2_hmac::<sha1::Sha1>(
            password,
            KEY_SALT,
            safe_storage::KEY_ITERATIONS,
            &mut key,
        );
        let mut buffer = plaintext.to_vec();
        buffer.resize(plaintext.len() + 16, 0);
        let ciphertext =
            cbc::Encryptor::<aes::Aes128>::new(&key.into(), &INITIALIZATION_VECTOR.into())
                .encrypt_padded_mut::<Pkcs7>(&mut buffer, plaintext.len())
                .unwrap();
        [prefix, ciphertext].concat()
    }

    #[test]
    fn decrypts_cookie_encrypted_without_keyring() {
        let encrypted = encrypt(b"v10", b"peanuts", b"xoxd-session");
        assert_eq!(decrypt_cookie(&encrypted).unwrap(), b"xoxd-session");
    }

    #[test]
    fn decryption_requires_matching_password() {
        let encrypted = encrypt(b"v10", b"peanuts", b"xoxd-session");
        let (_, ciphertext) = encrypted.split_at(ENCRYPTION_PREFIX_LENGTH);
        assert_eq!(
            decrypt_with_password(ciphertext, b"peanuts").unwrap(),
            b"xoxd-session"
        );
        assert_ne!(
            decrypt_with_password(ciphertext, b"wrong").as_deref(),
            Some(&b"xoxd-session"[..])
        );
    }

    #[test]
    fn rejects_unknown_encryption() {
        assert!(decrypt_cookie(b"v99abc").is_err());
        assert!(decrypt_cookie(b"v1").is_err());
    }
}
