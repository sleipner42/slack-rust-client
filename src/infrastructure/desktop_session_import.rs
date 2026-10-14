use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let plaintext = decrypt_cookie(&encrypted, &read_safe_storage_password()?)?;
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

fn decrypt_cookie(encrypted: &[u8], password: &str) -> Result<Vec<u8>, SlackError> {
    let ciphertext = encrypted
        .strip_prefix(ENCRYPTION_PREFIX)
        .ok_or_else(|| SlackError(String::from("unsupported cookie encryption")))?;
    let mut key = [0u8; KEY_LENGTH];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), KEY_SALT, KEY_ITERATIONS, &mut key);
    let mut buffer = ciphertext.to_vec();
    let plaintext = Aes128CbcDecryptor::new(&key.into(), &INITIALIZATION_VECTOR.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|_| SlackError(String::from("cookie decryption failed")))?;
    Ok(plaintext.to_vec())
}

fn read_safe_storage_password() -> Result<String, SlackError> {
    let output = Command::new(SECURITY_TOOL)
        .args(["find-generic-password", "-w", "-s", SAFE_STORAGE_SERVICE])
        .output()
        .map_err(|error| SlackError(error.to_string()))?;
    if !output.status.success() {
        return Err(SlackError(String::from(
            "Keychain access to Slack Safe Storage was denied",
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
const ENCRYPTION_PREFIX: &[u8] = b"v10";
const KEY_LENGTH: usize = 16;
const KEY_SALT: &[u8] = b"saltysalt";
const KEY_ITERATIONS: u32 = 1003;
const INITIALIZATION_VECTOR: [u8; 16] = [b' '; 16];
const SECURITY_TOOL: &str = "security";
const SAFE_STORAGE_SERVICE: &str = "Slack Safe Storage";
