//! Huddle audio in the app: Slack's huddles run on Amazon Chime, which this
//! joins as an audio-only attendee. Signaling follows the open-source
//! amazon-chime-sdk-js; media is WebRTC (webrtc-rs) carrying Opus.

mod audio;
mod protocol;
mod signaling;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use bytes::Bytes;
use rtc::media::Sample;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
    RtpCodecKind,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use webrtc::media_stream::MediaStreamTrack;
use webrtc::media_stream::Track;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer, RTCIceTransportPolicy,
    RTCPeerConnectionState, RTCSessionDescription, Registry, register_default_interceptors,
};

use crate::domain::model::ChimeJoin;
use protocol::{FrameType, SignalFrame};
use signaling::Incoming;

/// What a running call reports.
#[derive(Clone, Debug, PartialEq)]
pub enum CallUpdate {
    /// Audio flows both ways.
    Connected,
    /// Slack user IDs of those talking right now.
    Speaking(Vec<String>),
    /// The call is over; with what went wrong, if anything did.
    Ended(Option<String>),
}

enum Command {
    Mute(bool),
    Leave,
}

/// A call in progress; it ends when told to or when dropped.
pub struct Call {
    commands: UnboundedSender<Command>,
}

impl Call {
    /// Join the meeting and start talking. Updates arrive on `on_update`
    /// from the call's own thread.
    pub fn start(
        join: ChimeJoin,
        muted: bool,
        on_update: impl Fn(CallUpdate) + Send + Sync + 'static,
    ) -> Self {
        let (commands, received) = unbounded_channel();
        std::thread::Builder::new()
            .name(String::from("huddle-call"))
            .spawn(move || {
                let result = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())
                    .and_then(|runtime| runtime.block_on(run(join, muted, received, &on_update)));
                trace(&format!("ended: {result:?}"));
                on_update(CallUpdate::Ended(result.err()));
            })
            .expect("could not start the call thread");
        Self { commands }
    }

    pub fn set_muted(&self, muted: bool) {
        let _ = self.commands.send(Command::Mute(muted));
    }

    pub fn leave(&self) {
        let _ = self.commands.send(Command::Leave);
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.leave();
    }
}

struct Signals {
    send: mpsc::Sender<SignalFrame>,
    received: UnboundedReceiver<Incoming>,
}

impl Signals {
    fn send(&self, frame: SignalFrame) {
        let _ = self.send.send(frame);
    }

    /// The next frame, or why there is none.
    async fn next(&mut self) -> Result<SignalFrame, String> {
        match self.received.recv().await {
            Some(Incoming::Frame(frame)) => Ok(*frame),
            Some(Incoming::Closed(Some(signaling::MEETING_ENDED), _)) => {
                Err(String::from("The huddle has ended"))
            }
            Some(Incoming::Closed(code, reason)) => Err(format!(
                "The call's connection closed{}{}",
                code.map(|code| format!(" ({code})")).unwrap_or_default(),
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(": {reason}")
                }
            )),
            None => Err(String::from("The call's connection closed")),
        }
    }

    /// Wait for a frame of `kind`, answering pings and failing on errors.
    async fn expect(&mut self, kind: FrameType) -> Result<SignalFrame, String> {
        tokio::time::timeout(SETUP_TIMEOUT, async {
            loop {
                let frame = self.next().await?;
                if frame.kind() == Some(kind) {
                    return Ok(frame);
                }
                self.answer_common(&frame)?;
            }
        })
        .await
        .map_err(|_| String::from("The huddle did not answer in time"))?
    }

    /// Answer pings; fail on errors.
    fn answer_common(&self, frame: &SignalFrame) -> Result<(), String> {
        if let Some(error) = &frame.error {
            return Err(format!(
                "The huddle refused the call: {}",
                error
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("error {}", error.status.unwrap_or_default()))
            ));
        }
        if let Some(ping) = &frame.ping_pong
            && ping.r#type == protocol::PingPongType::Ping as i32
        {
            let mut pong = SignalFrame::new(FrameType::PingPong);
            pong.ping_pong = Some(protocol::PingPongFrame {
                r#type: protocol::PingPongType::Pong as i32,
                ping_id: ping.ping_id,
            });
            self.send(pong);
        }
        Ok(())
    }
}

async fn run(
    join: ChimeJoin,
    muted: bool,
    mut commands: UnboundedReceiver<Command>,
    on_update: &(dyn Fn(CallUpdate) + Send + Sync),
) -> Result<(), String> {
    let mut signals = open_signaling(&join).await?;
    trace("signaling connected");

    signals.send(join_frame());
    let join_ack = signals.expect(FrameType::JoinAck).await?;
    trace("joined");
    let turn = join_ack
        .joinack
        .and_then(|ack| ack.turn_credentials)
        .ok_or("The huddle sent no relay server")?;
    trace(&format!("relay servers: {:?}", turn.uris));
    signals.expect(FrameType::Index).await?;

    let state = Arc::new(Connection::default());
    let playback = audio::Playback::default();
    let peer = connect_media(&turn, state.clone(), playback.clone()).await?;
    let microphone = Arc::new(
        TrackLocalStaticSample::new(Instant::now(), opus_track())
            .map_err(|error| error.to_string())?,
    );
    let sender = peer
        .add_track(microphone.clone() as Arc<dyn TrackLocal>)
        .await
        .map_err(|error| error.to_string())?;

    let offer = peer
        .create_offer(None)
        .await
        .map_err(|error| error.to_string())?;
    peer.set_local_description(offer)
        .await
        .map_err(|error| error.to_string())?;
    let _ = tokio::time::timeout(GATHER_TIMEOUT, state.gathered.notified()).await;
    let offer = peer
        .local_description()
        .await
        .ok_or("No call offer was made")?;
    trace(&format!("offer:\n{}", offer.sdp));

    signals.send(subscribe_frame(&join, offer.sdp, muted));
    let ack = signals.expect(FrameType::SubscribeAck).await?;
    let answer = ack
        .suback
        .and_then(|ack| ack.sdp_answer)
        .filter(|sdp| !sdp.is_empty())
        .ok_or("The huddle sent no call answer")?;
    trace(&format!("answer:\n{answer}"));
    peer.set_remote_description(
        RTCSessionDescription::answer(answer).map_err(|error| error.to_string())?,
    )
    .await
    .map_err(|error| error.to_string())?;

    tokio::time::timeout(SETUP_TIMEOUT, state.connected.notified())
        .await
        .map_err(|_| String::from("The call's audio could not connect"))?;
    if state.failed.load(Ordering::SeqCst) {
        return Err(String::from("The call's audio could not connect"));
    }
    trace("media connected");

    let payload_type = sender
        .get_parameters()
        .await
        .map_err(|error| error.to_string())?
        .rtp_parameters
        .codecs
        .first()
        .map(|codec| codec.payload_type)
        .ok_or("No audio format was agreed")?;
    let ssrc = *microphone
        .ssrcs()
        .await
        .first()
        .ok_or("The microphone track has no stream")?;

    let muted = Arc::new(AtomicBool::new(muted));
    let (devices_stop, devices_stopped) = mpsc::channel::<()>();
    let frames = open_devices(playback, devices_stopped)?;
    let handle = tokio::runtime::Handle::current();
    let sending = muted.clone();
    std::thread::Builder::new()
        .name(String::from("huddle-microphone"))
        .spawn(move || send_microphone(frames, microphone, ssrc, payload_type, sending, handle))
        .map_err(|error| error.to_string())?;
    on_update(CallUpdate::Connected);

    let result = talk(&mut signals, &mut commands, &muted, &state, on_update).await;
    drop(devices_stop);
    let _ = peer.close().await;
    result
}

/// The call after it connected: until someone leaves or it breaks.
async fn talk(
    signals: &mut Signals,
    commands: &mut UnboundedReceiver<Command>,
    muted: &AtomicBool,
    state: &Connection,
    on_update: &(dyn Fn(CallUpdate) + Send + Sync),
) -> Result<(), String> {
    let mut ping = tokio::time::interval(PING_INTERVAL);
    let mut ping_id: u32 = 0;
    let mut speakers = Speakers::default();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Mute(now)) => {
                    muted.store(now, Ordering::SeqCst);
                    let mut frame = SignalFrame::new(FrameType::AudioControl);
                    frame.audio_control = Some(protocol::AudioControlFrame { muted: Some(now) });
                    signals.send(frame);
                }
                Some(Command::Leave) | None => {
                    trace("leaving");
                    let mut frame = SignalFrame::new(FrameType::Leave);
                    frame.leave = Some(protocol::LeaveFrame {});
                    signals.send(frame);
                    let _ = tokio::time::timeout(
                        Duration::from_secs(1),
                        signals.expect(FrameType::LeaveAck),
                    )
                    .await;
                    return Ok(());
                }
            },
            frame = signals.next() => {
                let frame = frame?;
                signals.answer_common(&frame)?;
                if let Some(info) = &frame.audio_stream_id_info {
                    speakers.learn(info);
                }
                if let Some(metadata) = &frame.audio_metadata
                    && let Some(speaking) = speakers.update(metadata)
                {
                    trace(&format!("talking: {speaking:?}"));
                    on_update(CallUpdate::Speaking(speaking));
                }
            }
            _ = ping.tick() => {
                ping_id = ping_id.wrapping_add(1);
                let mut frame = SignalFrame::new(FrameType::PingPong);
                frame.ping_pong = Some(protocol::PingPongFrame {
                    r#type: protocol::PingPongType::Ping as i32,
                    ping_id,
                });
                signals.send(frame);
            }
            _ = state.ended.notified() => {
                return Err(String::from("The call's audio connection was lost"));
            }
        }
    }
}

async fn open_signaling(join: &ChimeJoin) -> Result<Signals, String> {
    let (url, token) = (join.signaling_url.clone(), join.join_token.clone());
    let socket = tokio::task::spawn_blocking(move || signaling::connect(&url, &token))
        .await
        .map_err(|error| error.to_string())??;
    let (send, outgoing) = mpsc::channel();
    let (incoming, received) = unbounded_channel();
    std::thread::Builder::new()
        .name(String::from("huddle-signaling"))
        .spawn(move || signaling::run(socket, outgoing, incoming))
        .map_err(|error| error.to_string())?;
    Ok(Signals { send, received })
}

fn join_frame() -> SignalFrame {
    let mut frame = SignalFrame::new(FrameType::Join);
    frame.join = Some(protocol::JoinFrame {
        protocol_version: Some(2),
        max_num_of_videos: None,
        flags: Some(protocol::HAS_STREAM_UPDATE),
        client_details: Some(protocol::ClientDetails {
            app_name: Some(String::from("Better Slack")),
            app_version: Some(String::from(env!("CARGO_PKG_VERSION"))),
            platform_name: Some(String::from("webrtc-rs")),
            platform_version: Some(String::from("0.21")),
            client_source: Some(String::from("amazon-chime-sdk-js")),
            chime_sdk_version: Some(String::from("3.29.0")),
            client_utc_offset: Some(chrono::Local::now().format("%:z").to_string()),
            os_name: Some(String::from(std::env::consts::OS)),
        }),
        audio_session_id: Some(rand_u32() as u64),
        wants_compressed_sdp: Some(false),
    });
    frame
}

fn subscribe_frame(join: &ChimeJoin, sdp_offer: String, muted: bool) -> SignalFrame {
    let mut frame = SignalFrame::new(FrameType::Subscribe);
    frame.sub = Some(protocol::SubscribeFrame {
        duplex: Some(protocol::DUPLEX_RX),
        send_streams: vec![protocol::StreamDescriptor {
            stream_id: Some(1),
            framerate: Some(15),
            max_bitrate_kbps: Some(600),
            track_label: Some(String::from("AmazonChimeExpressAudio")),
            group_id: Some(1),
            avg_bitrate_bps: Some(400_000),
            attendee_id: Some(join.attendee_id.clone()),
            media_type: Some(protocol::MEDIA_AUDIO),
        }],
        receive_stream_ids: Vec::new(),
        sdp_offer: Some(sdp_offer),
        audio_host: Some(join.audio_host_url.clone()),
        audio_checkin: Some(false),
        audio_muted: Some(muted),
    });
    frame
}

/// How the peer connection is doing, for the code waiting on it.
#[derive(Default)]
struct Connection {
    gathered: tokio::sync::Notify,
    connected: tokio::sync::Notify,
    ended: tokio::sync::Notify,
    failed: AtomicBool,
    was_connected: AtomicBool,
}

struct Events {
    state: Arc<Connection>,
    playback: audio::Playback,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.state.gathered.notify_one();
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        trace(&format!("connection: {state}"));
        match state {
            RTCPeerConnectionState::Connected => {
                self.state.was_connected.store(true, Ordering::SeqCst);
                self.state.connected.notify_one();
            }
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                if self.state.was_connected.load(Ordering::SeqCst) {
                    self.state.ended.notify_one();
                } else {
                    self.state.failed.store(true, Ordering::SeqCst);
                    self.state.connected.notify_one();
                }
            }
            _ => {}
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        trace("receiving audio");
        let playback = self.playback.clone();
        tokio::spawn(play_track(track, playback));
    }
}

async fn connect_media(
    turn: &protocol::TurnCredentials,
    state: Arc<Connection>,
    playback: audio::Playback,
) -> Result<impl PeerConnection, String> {
    let mut media = MediaEngine::default();
    media
        .register_codec(
            RTCRtpCodecParameters {
                rtp_codec: opus_codec(),
                payload_type: 111,
            },
            RtpCodecKind::Audio,
        )
        .map_err(|error| error.to_string())?;
    let registry = register_default_interceptors(Registry::new(), &mut media)
        .map_err(|error| error.to_string())?;
    // Chime relays all media through its TURN servers, as its own SDK does.
    let urls: Vec<String> = turn
        .uris
        .iter()
        .filter(|uri| uri.starts_with("turn:"))
        .cloned()
        .collect();
    if urls.is_empty() {
        return Err(String::from("The huddle offered no usable relay server"));
    }
    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![RTCIceServer {
            urls,
            username: turn.username.clone().unwrap_or_default(),
            credential: turn.password.clone().unwrap_or_default(),
        }])
        .with_ice_transport_policy(RTCIceTransportPolicy::Relay)
        .build();
    let runtime = webrtc::runtime::default_runtime().ok_or("No async runtime")?;
    PeerConnectionBuilder::new()
        .with_configuration(config)
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .with_handler(Arc::new(Events { state, playback }))
        .with_runtime(runtime)
        .with_udp_addrs(vec![format!("{}:0", outgoing_address())])
        .build()
        .await
        .map_err(|error| format!("The call's audio could not start: {error}"))
}

/// The address this machine reaches the internet from. Binding only that
/// keeps container and VPN bridges from each reserving a relay.
fn outgoing_address() -> std::net::IpAddr {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            // Connecting a UDP socket sends nothing; it only picks the route.
            socket.connect("8.8.8.8:3478")?;
            socket.local_addr()
        })
        .map(|address| address.ip())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
}

fn opus_codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: String::from("audio/opus"),
        clock_rate: audio::RATE,
        channels: 2,
        sdp_fmtp_line: String::from("minptime=10;useinbandfec=1"),
        rtcp_feedback: vec![],
    }
}

fn opus_track() -> MediaStreamTrack {
    MediaStreamTrack::new(
        format!("better-slack-{}", rand_u32()),
        format!("microphone-{}", rand_u32()),
        String::from("Microphone"),
        RtpCodecKind::Audio,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(rand_u32()),
                ..Default::default()
            },
            codec: opus_codec(),
            ..Default::default()
        }],
    )
}

/// Start the devices on a thread that keeps them until `stop` is dropped.
fn open_devices(
    playback: audio::Playback,
    stop: mpsc::Receiver<()>,
) -> Result<mpsc::Receiver<Vec<f32>>, String> {
    let (opened, result) = mpsc::channel();
    std::thread::Builder::new()
        .name(String::from("huddle-audio"))
        .spawn(move || match audio::open(playback) {
            Ok((devices, frames)) => {
                let _ = opened.send(Ok(frames));
                let _ = stop.recv();
                drop(devices);
            }
            Err(error) => {
                let _ = opened.send(Err(error));
            }
        })
        .map_err(|error| error.to_string())?;
    result.recv().map_err(|error| error.to_string())?
}

/// Encode each 20 ms of microphone sound and send it; silence while muted.
fn send_microphone(
    frames: mpsc::Receiver<Vec<f32>>,
    track: Arc<TrackLocalStaticSample>,
    ssrc: u32,
    payload_type: u8,
    muted: Arc<AtomicBool>,
    runtime: tokio::runtime::Handle,
) {
    let Ok(mut encoder) =
        opus::Encoder::new(audio::RATE, opus::Channels::Mono, opus::Application::Voip)
    else {
        return;
    };
    let _ = encoder.set_inband_fec(true);
    let _ = encoder.set_dtx(true);
    let silence = vec![0.0; audio::FRAME];
    let mut packet = vec![0; 1500];
    while let Ok(frame) = frames.recv() {
        let input = if muted.load(Ordering::SeqCst) {
            &silence
        } else {
            &frame
        };
        let Ok(length) = encoder.encode_float(input, &mut packet) else {
            continue;
        };
        let sample = Sample {
            data: Bytes::copy_from_slice(&packet[..length]),
            duration: Duration::from_millis(20),
            ..Sample::new(Instant::now())
        };
        // A lost packet is not worth stopping for; the loop ends with the devices.
        let _ = runtime.block_on(
            track
                .sample_writer(ssrc, payload_type)
                .write_sample(&sample),
        );
    }
}

/// Decode the meeting's sound as it arrives and queue it for the speakers.
async fn play_track(track: Arc<dyn TrackRemote>, playback: audio::Playback) {
    let Ok(mut decoder) = opus::Decoder::new(audio::RATE, opus::Channels::Mono) else {
        return;
    };
    let mut pcm = vec![0.0; audio::FRAME * 6];
    let mut last_sequence: Option<u16> = None;
    while let Some(event) = track.poll().await {
        match event {
            TrackRemoteEvent::OnRtpPacket(packet) => {
                let sequence = packet.header.sequence_number;
                if let Some(last) = last_sequence {
                    let gap = sequence.wrapping_sub(last);
                    if gap == 0 || gap > u16::MAX / 2 {
                        continue; // A duplicate or a late packet.
                    }
                    // Conceal a few lost packets; more means the stream jumped.
                    for _ in 1..gap.min(4) {
                        if let Ok(count) =
                            decoder.decode_float(&[], &mut pcm[..audio::FRAME], false)
                        {
                            playback.push(&pcm[..count]);
                        }
                    }
                }
                last_sequence = Some(sequence);
                if let Ok(count) = decoder.decode_float(&packet.payload, &mut pcm, false) {
                    playback.push(&pcm[..count]);
                }
            }
            TrackRemoteEvent::OnEnded => break,
            _ => {}
        }
    }
}

/// Who is talking, from Chime's audio metadata.
#[derive(Default)]
struct Speakers {
    /// Chime's audio stream IDs to Slack user IDs.
    users: HashMap<u32, String>,
    speaking: Vec<String>,
}

impl Speakers {
    fn learn(&mut self, info: &protocol::AudioStreamIdInfoFrame) {
        for stream in &info.streams {
            let Some(id) = stream.audio_stream_id else {
                continue;
            };
            if stream.dropped == Some(true) {
                self.users.remove(&id);
            } else if let Some(user) = stream.external_user_id.as_deref().and_then(slack_user) {
                self.users.insert(id, user);
            }
        }
    }

    /// The new set of speakers, when it changed.
    fn update(&mut self, metadata: &protocol::AudioMetadataFrame) -> Option<Vec<String>> {
        let mut speaking: Vec<String> = metadata
            .attendee_states
            .iter()
            .filter(|state| state.muted != Some(true) && loudness(state.volume) > SPEAKING)
            .filter_map(|state| self.users.get(&state.audio_stream_id?).cloned())
            .collect();
        speaking.sort();
        speaking.dedup();
        (speaking != self.speaking).then(|| {
            self.speaking = speaking.clone();
            speaking
        })
    }
}

/// Slack's external user IDs look like `T…-R…-U…`; the last part is the user.
fn slack_user(external: &str) -> Option<String> {
    external
        .rsplit('-')
        .next()
        .filter(|user| user.starts_with('U') || user.starts_with('W'))
        .map(str::to_owned)
}

/// Chime's volume as 0 to 1, the way its SDK scales it.
fn loudness(volume: Option<u32>) -> f32 {
    let Some(volume) = volume else {
        return 0.0;
    };
    let decibels = -(volume as f32);
    (1.0 - (decibels - MAX_DECIBELS) / (MIN_DECIBELS - MAX_DECIBELS)).clamp(0.0, 1.0)
}

/// With `BETTER_SLACK_CALL_LOG` set, how the call gets going, on stderr.
fn trace(step: &str) {
    if std::env::var_os("BETTER_SLACK_CALL_LOG").is_some() {
        eprintln!("[huddle] {step}");
    }
}

fn rand_u32() -> u32 {
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

const SETUP_TIMEOUT: Duration = Duration::from_secs(15);
const GATHER_TIMEOUT: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(10);
const MIN_DECIBELS: f32 = -42.0;
const MAX_DECIBELS: f32 = -14.0;
const SPEAKING: f32 = 0.2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speakers_are_the_loud_unmuted_attendees() {
        let mut speakers = Speakers::default();
        speakers.learn(&protocol::AudioStreamIdInfoFrame {
            streams: vec![
                protocol::AudioStreamIdInfo {
                    audio_stream_id: Some(1),
                    external_user_id: Some(String::from("T1-R2-U111")),
                    ..Default::default()
                },
                protocol::AudioStreamIdInfo {
                    audio_stream_id: Some(2),
                    external_user_id: Some(String::from("T1-R2-U222")),
                    ..Default::default()
                },
            ],
        });
        let state = |id, volume, muted| protocol::AudioAttendeeState {
            audio_stream_id: Some(id),
            volume: Some(volume),
            muted: Some(muted),
            signal_strength: None,
        };
        let loud = protocol::AudioMetadataFrame {
            attendee_states: vec![state(1, 14, false), state(2, 14, true)],
        };
        assert_eq!(speakers.update(&loud), Some(vec![String::from("U111")]));
        assert_eq!(speakers.update(&loud), None, "only changes are reported");
        let quiet = protocol::AudioMetadataFrame {
            attendee_states: vec![state(1, 42, false)],
        };
        assert_eq!(speakers.update(&quiet), Some(Vec::new()));
        assert_eq!(slack_user("T1-R2-U3"), Some(String::from("U3")));
        assert_eq!(slack_user("attendee"), None);
    }
}
