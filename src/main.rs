mod application;
mod domain;
mod infrastructure;
mod ui;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, exit};
use std::sync::Arc;

use application::chat_service::ChatService;
use domain::model::Credentials;
use infrastructure::credential_store::CredentialStore;
use infrastructure::desktop_session_import::import_desktop_session;
use infrastructure::slack_api::SlackHttpGateway;
use infrastructure::webview_login::run_login_window;

fn main() -> eframe::Result {
    let store = CredentialStore::new(credentials_path());
    if env::args().nth(1).as_deref() == Some(LOGIN_COMMAND) {
        run_login_window(store);
    }
    let credentials = store
        .load()
        .or_else(|| import_from_desktop_app(&store))
        .unwrap_or_else(|| sign_in(&store));
    let team_name = credentials.team_name.clone();
    let gateway = Arc::new(SlackHttpGateway::new(credentials));
    let realtime = Arc::new(infrastructure::slack_realtime::SlackRealtime::new(
        Arc::clone(&gateway),
    ));
    let service = ChatService::new(gateway);
    ui::app::run(service, team_name, Some(realtime))
}

fn import_from_desktop_app(store: &CredentialStore) -> Option<Credentials> {
    let slack_data_dir = dirs::data_dir()?.join(SLACK_DATA_DIRECTORY);
    let scratch_dir = app_directory().join(IMPORT_SCRATCH_DIRECTORY);
    fs::create_dir_all(&scratch_dir).ok()?;
    let result = import_desktop_session(&slack_data_dir, &scratch_dir);
    let _ = fs::remove_dir_all(&scratch_dir);
    match result {
        Ok(credentials) => {
            store.save(&credentials).ok()?;
            Some(credentials)
        }
        Err(error) => {
            eprintln!("Desktop session import failed: {error}");
            None
        }
    }
}

fn sign_in(store: &CredentialStore) -> Credentials {
    let executable = env::current_exe().expect("cannot locate executable");
    let status = Command::new(executable)
        .arg(LOGIN_COMMAND)
        .status()
        .expect("failed to start login window");
    match (status.success(), store.load()) {
        (true, Some(credentials)) => credentials,
        _ => {
            eprintln!("Sign-in was not completed");
            exit(1)
        }
    }
}

fn credentials_path() -> PathBuf {
    app_directory().join(CREDENTIALS_FILE)
}

fn app_directory() -> PathBuf {
    dirs::config_dir()
        .expect("no config directory")
        .join(APP_DIRECTORY)
}

const LOGIN_COMMAND: &str = "login";
const APP_DIRECTORY: &str = "better-slack";
const CREDENTIALS_FILE: &str = "credentials.json";
const SLACK_DATA_DIRECTORY: &str = "Slack";
const IMPORT_SCRATCH_DIRECTORY: &str = "import";
