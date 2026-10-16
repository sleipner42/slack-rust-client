use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message as Frame, WebSocket};

use crate::domain::gateway::RealtimeSource;
use crate::domain::model::{LiveEvent, SlackError};
use crate::infrastructure::slack_api::SlackHttpGateway;
use crate::infrastructure::slack_parsing::parse_live_event;

pub struct SlackRealtime {
    gateway: Arc<SlackHttpGateway>,
}

impl SlackRealtime {
    pub fn new(gateway: Arc<SlackHttpGateway>) -> Self {
        Self { gateway }
    }

    fn connect(&self) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, SlackError> {
        let url = self.gateway.realtime_url()?;
        let mut request = url.into_client_request().map_err(to_error)?;
        let headers = request.headers_mut();
        headers.insert(
            "Cookie",
            HeaderValue::from_str(&format!("d={}", self.gateway.session_cookie()))
                .map_err(to_error)?,
        );
        headers.insert("Origin", HeaderValue::from_static(ORIGIN));
        let (mut socket, _) = tungstenite::connect(request).map_err(to_error)?;
        set_read_timeout(&mut socket, READ_TIMEOUT)?;
        Ok(socket)
    }
}

impl RealtimeSource for SlackRealtime {
    fn listen(&self, on_event: &mut dyn FnMut(LiveEvent)) -> Result<(), SlackError> {
        let mut socket = self.connect()?;
        let mut last_ping = Instant::now();
        let mut last_frame = Instant::now();
        let mut next_id: u64 = 1;
        loop {
            match socket.read() {
                Ok(Frame::Text(text)) => {
                    last_frame = Instant::now();
                    let frame = serde_json::from_str::<Value>(&text).ok();
                    if frame
                        .as_ref()
                        .is_some_and(|frame| frame["type"] == "goodbye")
                    {
                        return Err(SlackError(String::from("realtime server is restarting")));
                    }
                    if let Some(event) = frame.as_ref().and_then(parse_live_event) {
                        on_event(event);
                    }
                }
                Ok(Frame::Close(_)) => {
                    return Err(SlackError(String::from("realtime connection closed")));
                }
                Ok(_) => last_frame = Instant::now(),
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(error) => return Err(to_error(error)),
            }
            // Slack answers every ping; silence means a dead connection, which
            // after sleep or a network change can otherwise go unnoticed for long.
            if last_frame.elapsed() >= SILENCE_LIMIT {
                return Err(SlackError(String::from("realtime connection went silent")));
            }
            if last_ping.elapsed() >= PING_INTERVAL {
                let ping = json!({ "id": next_id, "type": "ping" }).to_string();
                socket.send(Frame::Text(ping.into())).map_err(to_error)?;
                next_id += 1;
                last_ping = Instant::now();
            }
        }
    }
}

fn set_read_timeout(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
) -> Result<(), SlackError> {
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => stream.get_mut(),
        _ => return Ok(()),
    };
    stream.set_read_timeout(Some(timeout)).map_err(to_error)
}

fn to_error(error: impl std::fmt::Display) -> SlackError {
    SlackError(error.to_string())
}

const ORIGIN: &str = "https://app.slack.com";
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(25);
const SILENCE_LIMIT: Duration = Duration::from_secs(70);
