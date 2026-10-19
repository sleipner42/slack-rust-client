use crate::domain::message_text::mentions_user;
use crate::domain::model::{Author, Message, ReadState, ts_seconds};

pub fn record_incoming(
    state: &mut ReadState,
    message: &Message,
    is_direct: bool,
    self_user_id: &str,
) {
    // Replayed events after reconnect must not increment mention badges twice.
    if !is_newer(&message.ts, state.latest_ts.as_deref()) {
        return;
    }
    state.latest_ts = Some(message.ts.clone());
    if message.author == Author::User(self_user_id.to_string()) {
        record_read(state, &message.ts);
        return;
    }
    if !is_newer(&message.ts, state.last_read_ts.as_deref()) {
        return;
    }
    state.has_unreads = true;
    if is_direct || mentions_user(&message.text, self_user_id) {
        state.mention_count += 1;
    }
}

pub fn record_read(state: &mut ReadState, ts: &str) {
    if is_newer(ts, state.last_read_ts.as_deref()) || state.last_read_ts.is_none() {
        state.last_read_ts = Some(ts.to_string());
    }
    let caught_up = state
        .latest_ts
        .as_deref()
        .is_none_or(|latest| !is_newer(latest, state.last_read_ts.as_deref()));
    if caught_up {
        state.has_unreads = false;
        state.mention_count = 0;
    }
}

pub fn is_active_since(state: &ReadState, cutoff_seconds: f64) -> bool {
    state.has_unreads
        || state.mention_count > 0
        || state
            .latest_ts
            .as_deref()
            .is_some_and(|latest| ts_seconds(latest) >= cutoff_seconds)
}

fn is_newer(ts: &str, reference: Option<&str>) -> bool {
    reference.is_none_or(|reference| ts_seconds(ts) > ts_seconds(reference))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(ts: &str, author: &str, text: &str) -> Message {
        Message {
            client_msg_id: None,
            ts: ts.into(),
            author: Author::User(author.into()),
            text: text.into(),
            thread_ts: None,
            reply_count: 0,
            reactions: Vec::new(),
            attachments: Vec::new(),
            shares: Vec::new(),
            edited: false,
            huddle: None,
        }
    }

    #[test]
    fn counts_mentions_and_clears_on_read() {
        let mut state = ReadState {
            last_read_ts: Some("10.0".into()),
            ..ReadState::default()
        };
        record_incoming(
            &mut state,
            &message("11.0", "U2", "hi <@UME>"),
            false,
            "UME",
        );
        record_incoming(&mut state, &message("12.0", "U2", "plain"), false, "UME");
        assert!(state.has_unreads);
        assert_eq!(state.mention_count, 1);
        record_read(&mut state, "12.0");
        assert!(!state.has_unreads);
        assert_eq!(state.mention_count, 0);
    }

    #[test]
    fn own_messages_mark_read() {
        let mut state = ReadState {
            has_unreads: true,
            mention_count: 2,
            ..ReadState::default()
        };
        record_incoming(&mut state, &message("20.0", "UME", "reply"), true, "UME");
        assert!(!state.has_unreads);
        assert_eq!(state.mention_count, 0);
    }

    #[test]
    fn direct_messages_count_as_mentions() {
        let mut state = ReadState::default();
        record_incoming(&mut state, &message("5.0", "U2", "yo"), true, "UME");
        assert_eq!(state.mention_count, 1);
    }

    #[test]
    fn replayed_events_do_not_duplicate_mentions_or_reopen_read_messages() {
        let mut state = ReadState::default();
        let incoming = message("5.0", "U2", "hi <@UME>");
        record_incoming(&mut state, &incoming, false, "UME");
        record_incoming(&mut state, &incoming, false, "UME");
        assert_eq!(state.mention_count, 1);
        record_read(&mut state, "5.0");
        record_incoming(&mut state, &incoming, false, "UME");
        assert!(!state.has_unreads);
    }

    #[test]
    fn older_read_acknowledgement_does_not_clear_newer_unreads() {
        let mut state = ReadState::default();
        record_incoming(&mut state, &message("10.0", "U2", "hi"), true, "UME");
        record_read(&mut state, "9.0");
        assert!(state.has_unreads);
        record_read(&mut state, "10.0");
        record_read(&mut state, "8.0");
        assert_eq!(state.last_read_ts.as_deref(), Some("10.0"));
        assert!(!state.has_unreads);
    }
}
