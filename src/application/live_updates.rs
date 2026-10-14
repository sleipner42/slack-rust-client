use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::domain::gateway::RealtimeSource;
use crate::domain::model::{LiveEvent, SlackError};

pub enum LiveStatus {
    Event(Box<LiveEvent>),
    Disconnected(SlackError),
}

pub fn spawn_live_updates(
    source: Arc<dyn RealtimeSource>,
    mut on_status: impl FnMut(LiveStatus) + Send + 'static,
) {
    thread::spawn(move || {
        let mut backoff = INITIAL_BACKOFF;
        loop {
            let mut connected = false;
            let result = source.listen(&mut |event| {
                connected = true;
                on_status(LiveStatus::Event(Box::new(event)));
            });
            if connected {
                backoff = INITIAL_BACKOFF;
            }
            if let Err(error) = result {
                on_status(LiveStatus::Disconnected(error));
            }
            thread::sleep(backoff);
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    });
}

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
