use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::model::Credentials;

pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Option<Credentials> {
        let content = fs::read_to_string(&self.path).ok()?;
        let stored: StoredCredentials = serde_json::from_str(&content).ok()?;
        Some(stored.into())
    }

    pub fn save(&self, credentials: &Credentials) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&StoredCredentials::from(credentials))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(OWNER_ONLY)
            .open(&self.path)?;
        file.write_all(content.as_bytes())
    }
}

#[derive(Serialize, Deserialize)]
struct StoredCredentials {
    token: String,
    cookie: String,
    workspace_url: String,
    team_name: String,
}

impl From<StoredCredentials> for Credentials {
    fn from(stored: StoredCredentials) -> Self {
        Self {
            token: stored.token,
            cookie: stored.cookie,
            workspace_url: stored.workspace_url,
            team_name: stored.team_name,
        }
    }
}

impl From<&Credentials> for StoredCredentials {
    fn from(credentials: &Credentials) -> Self {
        Self {
            token: credentials.token.clone(),
            cookie: credentials.cookie.clone(),
            workspace_url: credentials.workspace_url.clone(),
            team_name: credentials.team_name.clone(),
        }
    }
}

const OWNER_ONLY: u32 = 0o600;
