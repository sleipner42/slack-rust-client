use serde::Deserialize;
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

use crate::domain::model::Credentials;
use crate::infrastructure::credential_store::CredentialStore;

pub fn run_login_window(store: CredentialStore) -> ! {
    let event_loop = EventLoopBuilder::<TeamSession>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title(WINDOW_TITLE)
        .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT))
        .build(&event_loop)
        .expect("failed to create login window");
    let webview = WebViewBuilder::new()
        .with_url(SIGNIN_URL)
        .with_user_agent(USER_AGENT)
        .with_initialization_script(SESSION_PROBE_SCRIPT)
        .with_ipc_handler(move |request| {
            if let Ok(session) = serde_json::from_str::<TeamSession>(request.body()) {
                let _ = proxy.send_event(session);
            }
        })
        .build(&window)
        .expect("failed to create login webview");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(session) => {
                if let Some(cookie) = find_session_cookie(&webview) {
                    let credentials = session.into_credentials(cookie);
                    store
                        .save(&credentials)
                        .expect("failed to save credentials");
                    *control_flow = ControlFlow::ExitWithCode(0);
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::ExitWithCode(1),
            _ => {}
        }
    })
}

#[derive(Deserialize)]
struct TeamSession {
    token: String,
    url: String,
    name: String,
}

impl TeamSession {
    fn into_credentials(self, cookie: String) -> Credentials {
        Credentials {
            token: self.token,
            cookie,
            workspace_url: self.url,
            team_name: self.name,
        }
    }
}

fn find_session_cookie(webview: &wry::WebView) -> Option<String> {
    webview
        .cookies()
        .ok()?
        .into_iter()
        .find(|cookie| {
            cookie.name() == SESSION_COOKIE_NAME
                && cookie
                    .domain()
                    .is_some_and(|domain| domain.ends_with(SLACK_DOMAIN))
        })
        .map(|cookie| cookie.value().to_string())
}

const WINDOW_TITLE: &str = "Sign in to Slack";
const WINDOW_WIDTH: f64 = 900.0;
const WINDOW_HEIGHT: f64 = 750.0;
const SIGNIN_URL: &str = "https://app.slack.com/client";
const SESSION_COOKIE_NAME: &str = "d";
const SLACK_DOMAIN: &str = "slack.com";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 Safari/605.1.15";
const SESSION_PROBE_SCRIPT: &str = r#"
(function () {
  const timer = setInterval(function () {
    try {
      const config = JSON.parse(localStorage.getItem("localConfig_v2") || "null");
      if (!config || !config.teams) return;
      const teamId = config.lastActiveTeamId || Object.keys(config.teams)[0];
      const team = config.teams[teamId];
      if (!team || !team.token) return;
      clearInterval(timer);
      window.ipc.postMessage(JSON.stringify({ token: team.token, url: team.url, name: team.name }));
    } catch (e) {}
  }, 1000);
})();
"#;
