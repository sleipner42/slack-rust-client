//! The signaling WebSocket to Chime, on a thread of its own: frames to send
//! go in through a channel and received frames come out through another.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message as Frame, WebSocket};

use super::protocol::SignalFrame;

pub enum Incoming {
    Frame(Box<SignalFrame>),
    /// The socket closed, with Chime's close code (4410: the meeting ended).
    Closed(Option<u16>, String),
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

pub fn connect(signaling_url: &str, join_token: &str) -> Result<Socket, String> {
    let url = format!(
        "{signaling_url}?X-Chime-Control-Protocol-Version=3&X-Amzn-Chime-Send-Close-On-Error=1"
    );
    let mut request = url
        .into_client_request()
        .map_err(|error| error.to_string())?;
    let headers = request.headers_mut();
    headers.insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("_aws_wt_session, {join_token}"))
            .map_err(|error| error.to_string())?,
    );
    headers.insert("Origin", HeaderValue::from_static("https://app.slack.com"));
    let (mut socket, _) =
        tungstenite::connect(request).map_err(|error| format!("Signaling failed: {error}"))?;
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => stream.get_mut(),
        _ => return Err(String::from("Unexpected signaling stream")),
    };
    stream
        .set_read_timeout(Some(POLL))
        .map_err(|error| error.to_string())?;
    Ok(socket)
}

/// Pass frames both ways until either side goes away.
pub fn run(
    mut socket: Socket,
    outgoing: Receiver<SignalFrame>,
    incoming: UnboundedSender<Incoming>,
) {
    loop {
        loop {
            match outgoing.try_recv() {
                Ok(frame) => {
                    if let Err(error) = socket.send(Frame::Binary(frame.to_wire().into())) {
                        let _ = incoming.send(Incoming::Closed(None, error.to_string()));
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    return;
                }
            }
        }
        match socket.read() {
            Ok(Frame::Binary(bytes)) => {
                let Some(frame) = SignalFrame::from_wire(&bytes) else {
                    super::trace(&format!("undecodable frame of {} bytes", bytes.len()));
                    continue;
                };
                if incoming.send(Incoming::Frame(Box::new(frame))).is_err() {
                    let _ = socket.close(None);
                    return;
                }
            }
            Ok(Frame::Close(close)) => {
                let (code, reason) = close
                    .map(|close| (Some(u16::from(close.code)), close.reason.to_string()))
                    .unwrap_or((None, String::new()));
                let _ = incoming.send(Incoming::Closed(code, reason));
                return;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => {
                let _ = incoming.send(Incoming::Closed(None, error.to_string()));
                return;
            }
        }
    }
}

/// How long a read waits before outgoing frames get their turn.
const POLL: Duration = Duration::from_millis(20);

/// Chime closes with this when the meeting has ended.
pub const MEETING_ENDED: u16 = 4410;
