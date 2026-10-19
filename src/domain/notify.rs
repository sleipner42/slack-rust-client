use crate::domain::message_text::mentions_user;
use crate::domain::model::{Author, Message};

/// Slack's default desktop rule: direct messages, and mentions of you, @here,
/// @channel or @everyone elsewhere. Never your own messages or muted channels.
pub fn should_notify(message: &Message, is_direct: bool, muted: bool, self_user_id: &str) -> bool {
    let own = match &message.author {
        Author::User(id) => id == self_user_id,
        _ => false,
    };
    !own && !muted && (is_direct || mentions_user(&message.text, self_user_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(author: &str, text: &str) -> Message {
        Message {
            client_msg_id: None,
            ts: "1.0".into(),
            author: Author::User(author.into()),
            text: text.into(),
            thread_ts: None,
            reply_count: 0,
            reactions: vec![],
            attachments: vec![],
            shares: vec![],
            edited: false,
            huddle: None,
        }
    }

    #[test]
    fn direct_messages_and_mentions_notify() {
        assert!(should_notify(&message("U2", "hi"), true, false, "me"));
        assert!(should_notify(
            &message("U2", "<@me> look"),
            false,
            false,
            "me"
        ));
        assert!(should_notify(
            &message("U2", "<!here> deploy"),
            false,
            false,
            "me"
        ));
        assert!(!should_notify(
            &message("U2", "chatter"),
            false,
            false,
            "me"
        ));
        assert!(!should_notify(
            &message("me", "<@me> note to self"),
            true,
            false,
            "me"
        ));
        assert!(!should_notify(
            &message("U2", "<@me> muted"),
            false,
            true,
            "me"
        ));
    }
}
