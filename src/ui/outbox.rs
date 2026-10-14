use std::borrow::Cow;

use crate::application::chat_service::DisplayMessage;
use crate::domain::model::SlackError;

use super::composer::ComposeTarget;

#[derive(Clone)]
pub struct SendRequest {
    pub id: String,
    pub target: ComposeTarget,
    pub text: String,
    pub uploads: crate::domain::uploads::UploadBatch,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Delivery {
    Sending,
    Failed(String),
    Confirmed,
}

impl Delivery {
    pub fn layout_key(&self) -> u8 {
        match self {
            Self::Sending => 1,
            Self::Failed(_) => 2,
            Self::Confirmed => 0,
        }
    }
}

struct Outgoing {
    request: SendRequest,
    message: DisplayMessage,
    delivery: Delivery,
}

#[derive(Default)]
pub struct Outbox {
    items: Vec<Outgoing>,
}

impl Outbox {
    pub fn insert(&mut self, request: SendRequest, message: DisplayMessage) {
        self.items.push(Outgoing {
            request,
            message,
            delivery: Delivery::Sending,
        });
    }

    pub fn finish(&mut self, id: &str, result: Result<String, SlackError>) {
        // A live/history echo may already have confirmed and removed this row.
        let Some(item) = self.items.iter_mut().find(|item| item.request.id == id) else {
            return;
        };
        match result {
            Ok(ts) => {
                if !ts.is_empty() {
                    item.message.ts = ts;
                }
                item.delivery = Delivery::Confirmed;
            }
            Err(error) => item.delivery = Delivery::Failed(error.to_string()),
        }
    }

    pub fn update_uploads(&mut self, request: &SendRequest) {
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.request.id == request.id)
        {
            item.request.uploads = request.uploads.clone();
            for (attachment, id) in item
                .message
                .attachments
                .iter_mut()
                .zip(&request.uploads.uploaded_ids)
            {
                attachment.file_id = Some(id.clone());
            }
        }
    }

    pub fn retry(&mut self, id: &str) -> Option<SendRequest> {
        let item = self.items.iter_mut().find(|item| item.request.id == id)?;
        if !matches!(item.delivery, Delivery::Failed(_)) {
            return None;
        }
        item.delivery = Delivery::Sending;
        // Reuse the client ID, including after an ambiguous network timeout.
        Some(item.request.clone())
    }

    pub fn delivery(&self, message: &DisplayMessage) -> Option<&Delivery> {
        let id = message.client_msg_id.as_deref()?;
        self.items
            .iter()
            .find(|item| item.request.id == id)
            .map(|item| &item.delivery)
    }

    pub fn reconcile(&mut self, target: &ComposeTarget, messages: &[DisplayMessage]) {
        self.items.retain(|item| {
            &item.request.target != target || !messages.iter().any(|message| item.matches(message))
        });
    }

    pub fn messages<'a>(
        &self,
        target: &ComposeTarget,
        messages: &'a [DisplayMessage],
    ) -> Cow<'a, [DisplayMessage]> {
        let pending: Vec<_> = self
            .items
            .iter()
            .filter(|item| {
                &item.request.target == target
                    && !messages.iter().any(|message| item.matches(message))
            })
            .collect();
        if pending.is_empty() {
            return Cow::Borrowed(messages);
        }
        let mut combined = messages.to_vec();
        combined.extend(pending.iter().map(|item| item.message.clone()));
        combined.sort_by(|a, b| {
            crate::domain::model::ts_seconds(&a.ts)
                .total_cmp(&crate::domain::model::ts_seconds(&b.ts))
        });
        Cow::Owned(combined)
    }

    pub fn has_messages(&self, target: &ComposeTarget) -> bool {
        self.items.iter().any(|item| &item.request.target == target)
    }
}

impl Outgoing {
    fn matches(&self, message: &DisplayMessage) -> bool {
        message.client_msg_id.as_deref() == Some(&self.request.id)
            || (self.delivery == Delivery::Confirmed && message.ts == self.message.ts)
            || (!self.request.uploads.uploaded_ids.is_empty()
                && message.attachments.iter().any(|attachment| {
                    attachment
                        .file_id
                        .as_ref()
                        .is_some_and(|id| self.request.uploads.uploaded_ids.contains(id))
                }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::chat_service::ChatService;
    use crate::infrastructure::demo::DemoGateway;
    use std::sync::Arc;

    #[test]
    fn upload_echo_reconciles_by_file_id_without_a_client_message_id() {
        let (mut request, mut message) = fixture("upload", Some("10.0"));
        request
            .uploads
            .files
            .push(crate::domain::uploads::UploadFile {
                name: "notes.txt".into(),
                bytes: Vec::from(b"notes").into(),
            });
        message.attachments.push(crate::domain::model::Attachment {
            file_id: None,
            name: "notes.txt".into(),
            preview_url: None,
            full_url: None,
            permalink: None,
            width: None,
            height: None,
            preview_pending: false,
        });
        let mut outbox = Outbox::default();
        outbox.insert(request.clone(), message.clone());
        request.uploads.uploaded_ids.push("F1".into());
        request.uploads.completion_started = true;
        outbox.update_uploads(&request);
        outbox.finish(
            &request.id,
            Err(SlackError("confirmation interrupted".into())),
        );
        let retry = outbox.retry(&request.id).unwrap();
        assert_eq!(retry.uploads.uploaded_ids, ["F1"]);
        assert!(retry.uploads.completion_started);
        let mut echo = message;
        echo.ts = "50.0".into();
        echo.client_msg_id = None;
        echo.attachments[0].file_id = Some("F1".into());
        assert_eq!(outbox.messages(&request.target, &[echo.clone()]).len(), 1);
        outbox.reconcile(&request.target, &[echo]);
        assert!(!outbox.has_messages(&request.target));
    }

    fn fixture(id: &str, thread: Option<&str>) -> (SendRequest, DisplayMessage) {
        let service = ChatService::new(Arc::new(DemoGateway::new()));
        service.load_workspace().unwrap();
        let request = SendRequest {
            id: id.into(),
            target: ComposeTarget {
                conversation_id: "C1".into(),
                thread_ts: thread.map(str::to_owned),
            },
            text: "The same text\non two lines".into(),
            uploads: Default::default(),
        };
        let message = service.outgoing_message(&request.text, thread, id);
        (request, message)
    }

    #[test]
    fn pending_rows_survive_stale_history_and_reconcile_by_id_not_text() {
        let (first, first_message) = fixture("first", None);
        let (second, second_message) = fixture("second", None);
        let (thread, thread_message) = fixture("reply", Some("10.0"));
        let mut outbox = Outbox::default();
        outbox.insert(first.clone(), first_message.clone());
        outbox.insert(second.clone(), second_message.clone());
        outbox.insert(thread.clone(), thread_message);
        outbox.reconcile(&first.target, &[]);
        assert_eq!(outbox.messages(&first.target, &[]).len(), 2);
        assert_eq!(outbox.messages(&thread.target, &[]).len(), 1);
        let mut server = second_message;
        server.ts = "20.0".into();
        outbox.reconcile(&first.target, &[server.clone()]);
        let visible = outbox
            .messages(&first.target, &[server.clone()])
            .into_owned();
        assert_eq!(
            visible.len(),
            2,
            "identical text is still two different sends"
        );
        assert_eq!(
            visible
                .iter()
                .filter(|m| m.client_msg_id.as_deref() == Some("second"))
                .count(),
            1
        );
        outbox.finish(
            "second",
            Err(SlackError("late timeout after live echo".into())),
        );
        assert!(
            outbox.delivery(&server).is_none(),
            "late failure must not resurrect a delivered row"
        );
        assert_eq!(outbox.delivery(&first_message), Some(&Delivery::Sending));
    }

    #[test]
    fn retry_keeps_original_text_and_id_and_cannot_be_sent_twice() {
        let (request, message) = fixture("retry-id", Some("10.0"));
        let mut outbox = Outbox::default();
        outbox.insert(request.clone(), message.clone());
        assert!(outbox.retry(&request.id).is_none());
        outbox.finish(&request.id, Err(SlackError("offline".into())));
        assert!(matches!(
            outbox.delivery(&message),
            Some(Delivery::Failed(_))
        ));
        let retry = outbox.retry(&request.id).unwrap();
        assert_eq!(retry.id, request.id);
        assert_eq!(retry.text, request.text);
        assert_eq!(retry.target, request.target);
        assert!(outbox.retry(&request.id).is_none());
        outbox.finish(&request.id, Ok("30.0".into()));
        outbox.reconcile(&request.target, &[]);
        assert_eq!(outbox.messages(&request.target, &[])[0].ts, "30.0");
        let mut server = message;
        server.ts = "30.0".into();
        server.client_msg_id = None;
        outbox.reconcile(&request.target, &[server]);
        assert!(
            !outbox.has_messages(&request.target),
            "timestamp receipt also reconciles older history payloads"
        );
    }
}
