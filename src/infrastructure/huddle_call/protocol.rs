//! The part of Amazon Chime's signaling protocol an audio-only client needs,
//! after `SignalingProtocol.proto` in the Apache-2.0 amazon-chime-sdk-js.
//! Field numbers must match it exactly.

use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct SignalFrame {
    #[prost(uint64, required, tag = "1")]
    pub timestamp_ms: u64,
    #[prost(enumeration = "FrameType", required, tag = "2")]
    pub r#type: i32,
    #[prost(message, optional, tag = "3")]
    pub error: Option<ErrorFrame>,
    #[prost(message, optional, tag = "4")]
    pub join: Option<JoinFrame>,
    #[prost(message, optional, tag = "5")]
    pub joinack: Option<JoinAckFrame>,
    #[prost(message, optional, tag = "6")]
    pub sub: Option<SubscribeFrame>,
    #[prost(message, optional, tag = "7")]
    pub suback: Option<SubscribeAckFrame>,
    #[prost(message, optional, tag = "11")]
    pub leave: Option<LeaveFrame>,
    #[prost(message, optional, tag = "17")]
    pub audio_control: Option<AudioControlFrame>,
    #[prost(message, optional, tag = "18")]
    pub audio_metadata: Option<AudioMetadataFrame>,
    #[prost(message, optional, tag = "19")]
    pub audio_stream_id_info: Option<AudioStreamIdInfoFrame>,
    #[prost(message, optional, tag = "20")]
    pub ping_pong: Option<PingPongFrame>,
    #[prost(message, optional, tag = "21")]
    pub audio_status: Option<AudioStatusFrame>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum FrameType {
    Join = 1,
    JoinAck = 2,
    Subscribe = 3,
    SubscribeAck = 4,
    Index = 5,
    Pause = 7,
    Resume = 8,
    Leave = 9,
    LeaveAck = 10,
    Bitrates = 13,
    AudioControl = 16,
    AudioMetadata = 17,
    AudioStreamIdInfo = 18,
    PingPong = 19,
    AudioStatus = 20,
    ClientMetric = 21,
    DataMessage = 22,
    RemoteVideoUpdate = 24,
    PrimaryMeetingJoin = 25,
    PrimaryMeetingJoinAck = 26,
    PrimaryMeetingLeave = 27,
    Notification = 34,
    MeetingSessionTiming = 43,
}

#[derive(Clone, PartialEq, Message)]
pub struct ErrorFrame {
    #[prost(uint32, optional, tag = "1")]
    pub status: Option<u32>,
    #[prost(string, optional, tag = "2")]
    pub description: Option<String>,
}

/// `SdkJoinFlags.HAS_STREAM_UPDATE`.
pub const HAS_STREAM_UPDATE: u32 = 2;

#[derive(Clone, PartialEq, Message)]
pub struct ClientDetails {
    #[prost(string, optional, tag = "1")]
    pub app_name: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub app_version: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub platform_name: Option<String>,
    #[prost(string, optional, tag = "6")]
    pub platform_version: Option<String>,
    #[prost(string, optional, tag = "7")]
    pub client_source: Option<String>,
    #[prost(string, optional, tag = "8")]
    pub chime_sdk_version: Option<String>,
    #[prost(string, optional, tag = "9")]
    pub client_utc_offset: Option<String>,
    #[prost(string, optional, tag = "10")]
    pub os_name: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct JoinFrame {
    #[prost(uint32, optional, tag = "1")]
    pub protocol_version: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub max_num_of_videos: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub flags: Option<u32>,
    #[prost(message, optional, tag = "4")]
    pub client_details: Option<ClientDetails>,
    #[prost(uint64, optional, tag = "6")]
    pub audio_session_id: Option<u64>,
    #[prost(bool, optional, tag = "7")]
    pub wants_compressed_sdp: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TurnCredentials {
    #[prost(string, optional, tag = "1")]
    pub username: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub password: Option<String>,
    #[prost(uint32, optional, tag = "3")]
    pub ttl: Option<u32>,
    #[prost(string, repeated, tag = "4")]
    pub uris: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct JoinAckFrame {
    #[prost(message, optional, tag = "1")]
    pub turn_credentials: Option<TurnCredentials>,
    #[prost(uint32, optional, tag = "2")]
    pub video_subscription_limit: Option<u32>,
    #[prost(bool, optional, tag = "3")]
    pub wants_compressed_sdp: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LeaveFrame {}

/// `SdkStreamServiceType.RX`: receive only, as for an audio-only attendee.
pub const DUPLEX_RX: i32 = 1;
/// `SdkStreamMediaType.AUDIO`.
pub const MEDIA_AUDIO: i32 = 1;

#[derive(Clone, PartialEq, Message)]
pub struct StreamDescriptor {
    #[prost(uint32, optional, tag = "1")]
    pub stream_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub framerate: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub max_bitrate_kbps: Option<u32>,
    #[prost(string, optional, tag = "4")]
    pub track_label: Option<String>,
    #[prost(uint32, optional, tag = "6")]
    pub group_id: Option<u32>,
    #[prost(uint32, optional, tag = "7")]
    pub avg_bitrate_bps: Option<u32>,
    #[prost(string, optional, tag = "8")]
    pub attendee_id: Option<String>,
    #[prost(int32, optional, tag = "9")]
    pub media_type: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SubscribeFrame {
    #[prost(int32, optional, tag = "1")]
    pub duplex: Option<i32>,
    #[prost(message, repeated, tag = "2")]
    pub send_streams: Vec<StreamDescriptor>,
    #[prost(uint32, repeated, packed = "false", tag = "3")]
    pub receive_stream_ids: Vec<u32>,
    #[prost(string, optional, tag = "4")]
    pub sdp_offer: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub audio_host: Option<String>,
    #[prost(bool, optional, tag = "6")]
    pub audio_checkin: Option<bool>,
    #[prost(bool, optional, tag = "7")]
    pub audio_muted: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SubscribeAckFrame {
    #[prost(int32, optional, tag = "1")]
    pub duplex: Option<i32>,
    #[prost(string, optional, tag = "3")]
    pub sdp_answer: Option<String>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub compressed_sdp_answer: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AudioControlFrame {
    #[prost(bool, optional, tag = "1")]
    pub muted: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AudioMetadataFrame {
    #[prost(message, repeated, tag = "1")]
    pub attendee_states: Vec<AudioAttendeeState>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AudioAttendeeState {
    #[prost(uint32, optional, tag = "1")]
    pub audio_stream_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub volume: Option<u32>,
    #[prost(bool, optional, tag = "3")]
    pub muted: Option<bool>,
    #[prost(uint32, optional, tag = "4")]
    pub signal_strength: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AudioStreamIdInfoFrame {
    #[prost(message, repeated, tag = "1")]
    pub streams: Vec<AudioStreamIdInfo>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AudioStreamIdInfo {
    #[prost(uint32, optional, tag = "1")]
    pub audio_stream_id: Option<u32>,
    #[prost(string, optional, tag = "2")]
    pub attendee_id: Option<String>,
    #[prost(bool, optional, tag = "3")]
    pub muted: Option<bool>,
    #[prost(string, optional, tag = "4")]
    pub external_user_id: Option<String>,
    #[prost(bool, optional, tag = "5")]
    pub dropped: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum PingPongType {
    Ping = 1,
    Pong = 2,
}

#[derive(Clone, PartialEq, Message)]
pub struct PingPongFrame {
    #[prost(enumeration = "PingPongType", required, tag = "1")]
    pub r#type: i32,
    #[prost(uint32, required, tag = "2")]
    pub ping_id: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct AudioStatusFrame {
    #[prost(uint32, optional, tag = "1")]
    pub audio_status: Option<u32>,
}

/// Every frame on the socket starts with this byte.
pub const FRAME_TYPE_RTC: u8 = 0x5;

impl SignalFrame {
    pub fn new(kind: FrameType) -> Self {
        Self {
            timestamp_ms: now_ms(),
            r#type: kind as i32,
            ..Default::default()
        }
    }

    pub fn kind(&self) -> Option<FrameType> {
        FrameType::try_from(self.r#type).ok()
    }

    /// The frame as sent on the socket.
    pub fn to_wire(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.encoded_len() + 1);
        bytes.push(FRAME_TYPE_RTC);
        self.encode(&mut bytes).expect("a Vec grows as needed");
        bytes
    }

    /// A received frame. Chime marks its own with 0x02 rather than
    /// [`FRAME_TYPE_RTC`]; like the SDK, ignore the marker.
    pub fn from_wire(bytes: &[u8]) -> Option<Self> {
        Self::decode(bytes.get(1..)?).ok()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_with_the_rtc_prefix() {
        let mut frame = SignalFrame::new(FrameType::AudioControl);
        frame.audio_control = Some(AudioControlFrame { muted: Some(true) });
        let wire = frame.to_wire();
        assert_eq!(wire[0], FRAME_TYPE_RTC);
        // Field 2 (type) is varint 16: tag byte 0x10, value 0x10.
        assert!(wire.windows(2).any(|pair| pair == [0x10, 0x10]));
        let back = SignalFrame::from_wire(&wire).unwrap();
        assert_eq!(back.kind(), Some(FrameType::AudioControl));
        assert_eq!(back.audio_control.unwrap().muted, Some(true));
        // What Chime sends: the same frame marked 0x02.
        let mut from_server = wire.clone();
        from_server[0] = 0x02;
        assert_eq!(
            SignalFrame::from_wire(&from_server).unwrap().kind(),
            Some(FrameType::AudioControl)
        );
        assert!(SignalFrame::from_wire(&[]).is_none());
    }
}
