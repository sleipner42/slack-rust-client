//! Huddles: who is in one where, the incoming-call banner, the call bar
//! while in one, and the huddle card in conversations.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;

use super::{images::ImageCache, theme};
use crate::domain::model::{ChimeJoin, Huddle};
use crate::infrastructure::calls::Ringtone;
use crate::infrastructure::huddle_call::{Call, CallUpdate};

/// A huddle with the names of the people in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveHuddle {
    pub huddle: Huddle,
    /// Names of `huddle.participants`, once looked up.
    pub names: Vec<String>,
}

/// Someone calling the user.
pub struct Ringing {
    pub conversation_id: String,
    pub caller_id: String,
    pub caller: String,
    pub avatar_url: Option<String>,
    /// Slack's call ID, for declining.
    pub call_id: String,
    since: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CallChoice {
    Answer,
    Decline,
}

/// The huddle the user is in, from asking Slack to join until leaving.
pub struct ActiveCall {
    pub conversation_id: String,
    /// What the conversation is called, for the call bar.
    pub title: String,
    /// Slack's answer to joining; `None` while waiting for it.
    pub join: Option<ChimeJoin>,
    call: Option<Call>,
    /// When audio started flowing.
    pub connected: Option<Instant>,
    pub muted: bool,
    /// Slack user IDs of those talking.
    pub speaking: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CallAction {
    ToggleMute,
    Leave,
}

#[derive(Default)]
pub struct Huddles {
    /// The latest huddle per conversation, ended ones included.
    known: HashMap<String, LiveHuddle>,
    ringing: Option<Ringing>,
    ringtone: Ringtone,
    call: Option<ActiveCall>,
}

impl Huddles {
    pub fn known(&self) -> &HashMap<String, LiveHuddle> {
        &self.known
    }

    /// The huddle going on in a conversation, if any.
    pub fn live(&self, conversation_id: &str) -> Option<&LiveHuddle> {
        self.known
            .get(conversation_id)
            .filter(|live| live.huddle.is_live())
    }

    /// Slack's latest word on a huddle. Returns whether names need looking up.
    pub fn update(&mut self, huddle: Huddle, self_user_id: &str) -> bool {
        if self.ringing.as_ref().is_some_and(|ringing| {
            ringing.conversation_id == huddle.conversation_id
                && (huddle.ended || huddle.participants.iter().any(|id| id == self_user_id))
        }) {
            // Answered somewhere else, or the huddle is over.
            self.stop_ringing();
        }
        let entry = self
            .known
            .entry(huddle.conversation_id.clone())
            .or_default();
        let names_known = entry.huddle.participants == huddle.participants;
        if !names_known {
            entry.names.clear();
        }
        entry.huddle = huddle;
        !names_known && !entry.huddle.participants.is_empty()
    }

    /// A huddle seen in history; live events, which are newer, win.
    pub fn seen(&mut self, huddle: &Huddle, self_user_id: &str) -> bool {
        let newer = self.known.get(&huddle.conversation_id).is_none_or(|known| {
            known.huddle.id != huddle.id && known.huddle.started < huddle.started
        });
        newer && self.update(huddle.clone(), self_user_id)
    }

    pub fn named(&mut self, conversation_id: &str, participants: &[String], names: Vec<String>) {
        if let Some(live) = self
            .known
            .get_mut(conversation_id)
            .filter(|live| live.huddle.participants == participants)
        {
            live.names = names;
        }
    }

    pub fn ringing(&self) -> Option<&Ringing> {
        self.ringing.as_ref()
    }

    pub fn ring(&mut self, conversation_id: String, caller_id: String, call_id: String) {
        self.ringing = Some(Ringing {
            conversation_id,
            caller: caller_id.clone(),
            caller_id,
            avatar_url: None,
            call_id,
            since: Instant::now(),
        });
        self.ringtone.stop();
    }

    pub fn call(&self) -> Option<&ActiveCall> {
        self.call.as_ref()
    }

    /// Whether the user is in (or joining) this conversation's huddle.
    pub fn in_call(&self, conversation_id: &str) -> bool {
        self.call
            .as_ref()
            .is_some_and(|call| call.conversation_id == conversation_id)
    }

    /// Asking Slack to join; returns the call this replaces, to leave.
    pub fn joining(&mut self, conversation_id: String, title: String) -> Option<ActiveCall> {
        self.call.replace(ActiveCall {
            conversation_id,
            title,
            join: None,
            call: None,
            connected: None,
            muted: false,
            speaking: Vec::new(),
        })
    }

    /// Slack said how to join. Returns false when the user has moved on, and
    /// the call should be left again.
    pub fn joined(&mut self, join: &ChimeJoin) -> bool {
        match &mut self.call {
            Some(call) if call.join.is_none() && call.conversation_id == join.conversation_id => {
                call.join = Some(join.clone());
                true
            }
            _ => false,
        }
    }

    /// The audio is starting; it belongs to the call just joined.
    pub fn started(&mut self, audio: Call) {
        match &mut self.call {
            Some(call) => {
                audio.set_muted(call.muted);
                call.call = Some(audio);
            }
            None => audio.leave(),
        }
    }

    /// News from the call with this Slack call ID. Returns the call when it
    /// ended, so Slack can be told.
    pub fn call_update(&mut self, call_id: &str, update: CallUpdate) -> Option<ActiveCall> {
        let call = self.call.as_mut().filter(|call| {
            call.join
                .as_ref()
                .is_some_and(|join| join.call_id == call_id)
        })?;
        match update {
            CallUpdate::Connected => call.connected = Some(Instant::now()),
            CallUpdate::Speaking(speaking) => call.speaking = speaking,
            CallUpdate::Ended(_) => return self.call.take(),
        }
        None
    }

    /// Leave the huddle; returns the call so Slack can be told.
    pub fn leave(&mut self) -> Option<ActiveCall> {
        let call = self.call.take()?;
        if let Some(audio) = &call.call {
            audio.leave();
        }
        Some(call)
    }

    pub fn toggle_mute(&mut self) {
        if let Some(call) = &mut self.call {
            call.muted = !call.muted;
            if let Some(audio) = &call.call {
                audio.set_muted(call.muted);
            }
        }
    }

    pub fn caller_found(&mut self, caller_id: &str, name: String, avatar_url: Option<String>) {
        if let Some(ringing) = self
            .ringing
            .as_mut()
            .filter(|ringing| ringing.caller_id == caller_id)
        {
            ringing.caller = name;
            ringing.avatar_url = avatar_url;
        }
    }

    /// Stop ringing for a conversation; `None` stops any ring.
    pub fn stop_ringing_in(&mut self, conversation_id: Option<&str>) -> Option<Ringing> {
        if conversation_id.is_some_and(|id| {
            self.ringing
                .as_ref()
                .is_some_and(|ringing| ringing.conversation_id != id)
        }) {
            return None;
        }
        self.stop_ringing()
    }

    fn stop_ringing(&mut self) -> Option<Ringing> {
        self.ringtone.stop();
        self.ringing.take()
    }

    /// Keep the ringtone going; give up after Slack would have.
    pub fn tick(&mut self) -> Option<Duration> {
        if self.ringing.is_none() && self.call.is_some() {
            // The call bar's clock.
            return Some(Duration::from_secs(1));
        }
        let since = self.ringing.as_ref()?.since;
        if since.elapsed() >= RING_LIMIT {
            self.stop_ringing();
            return None;
        }
        self.ringtone.tick();
        Some(Duration::from_millis(500))
    }

    /// The incoming-call banner, above everything else.
    pub fn banner(&self, ctx: &egui::Context, images: &mut ImageCache) -> Option<CallChoice> {
        let ringing = self.ringing.as_ref()?;
        let mut choice = None;
        egui::Area::new(egui::Id::new("incoming_call"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 16.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_min_width(320.0);
                        ui.horizontal(|ui| {
                            super::activity::avatar(
                                ui,
                                ringing.avatar_url.as_deref(),
                                &ringing.caller,
                                images,
                            );
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(&ringing.caller)
                                        .strong()
                                        .color(theme::TEXT_STRONG),
                                );
                                ui.weak("is calling you in a huddle");
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add(egui::Button::new("Decline"))
                                        .on_hover_text("Let them know you can't talk now")
                                        .clicked()
                                    {
                                        choice = Some(CallChoice::Decline);
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("Answer")
                                                    .color(egui::Color32::WHITE),
                                            )
                                            .fill(ANSWER),
                                        )
                                        .on_hover_text("Join the huddle")
                                        .clicked()
                                    {
                                        choice = Some(CallChoice::Answer);
                                    }
                                },
                            );
                        });
                    });
            });
        choice
    }
}

/// The call bar at the bottom of the sidebar while in a huddle. `names`
/// gives names for user IDs where known.
pub fn call_bar(
    ui: &mut egui::Ui,
    call: &ActiveCall,
    names: &HashMap<String, String>,
) -> Option<CallAction> {
    let mut action = None;
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                let color = if call.connected.is_some() {
                    ANSWER_LIGHT
                } else {
                    theme::TEXT_MUTED
                };
                paint_headphones(ui, rect, color);
                ui.vertical(|ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&call.title)
                                .strong()
                                .color(theme::TEXT_STRONG),
                        )
                        .truncate(),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(call_status(call, names)).small().weak(),
                        )
                        .truncate(),
                    );
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let mute = if call.muted { "Unmute" } else { "Mute" };
                let button = egui::Button::new(mute);
                let button = if call.muted {
                    button.fill(theme::SELF_MENTION_BACKGROUND)
                } else {
                    button
                };
                if ui
                    .add(button)
                    .on_hover_text(if call.muted {
                        "Others cannot hear you"
                    } else {
                        "Turn your microphone off"
                    })
                    .clicked()
                {
                    action = Some(CallAction::ToggleMute);
                }
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("Leave").color(egui::Color32::WHITE))
                            .fill(theme::DANGER),
                    )
                    .on_hover_text("Leave the huddle")
                    .clicked()
                {
                    action = Some(CallAction::Leave);
                }
            });
        });
    action
}

fn call_status(call: &ActiveCall, names: &HashMap<String, String>) -> String {
    let Some(connected) = call.connected else {
        return "Connecting…".into();
    };
    let talking: Vec<&str> = call
        .speaking
        .iter()
        .filter_map(|id| names.get(id).map(String::as_str))
        .collect();
    let seconds = connected.elapsed().as_secs();
    let clock = format!("{}:{:02}", seconds / 60, seconds % 60);
    match talking.as_slice() {
        [] => clock,
        [one] => format!("{clock} · {one} is talking"),
        many => format!("{clock} · {} are talking", many.join(", ")),
    }
}

/// Slack rings for about half a minute; this is a fallback if the stop is lost.
const RING_LIMIT: Duration = Duration::from_secs(60);
const ANSWER: egui::Color32 = egui::Color32::from_rgb(0, 122, 90);

/// A huddle in the conversation: who is in it and a Join button, or how long
/// it lasted. Returns true when Join is clicked.
pub fn huddle_card(
    ui: &mut egui::Ui,
    snapshot: &Huddle,
    live: Option<&LiveHuddle>,
    in_call: bool,
) -> bool {
    let huddle = live.map_or(snapshot, |live| &live.huddle);
    let mut join = false;
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(420.0));
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                let color = if huddle.is_live() {
                    ANSWER_LIGHT
                } else {
                    theme::TEXT_MUTED
                };
                paint_headphones(ui, rect, color);
                ui.vertical(|ui| {
                    let (title, detail) = card_text(huddle, live.map(|live| live.names.as_slice()));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title)
                                .strong()
                                .color(theme::TEXT_STRONG),
                        )
                        .truncate(),
                    );
                    ui.add(egui::Label::new(egui::RichText::new(detail).small().weak()).truncate());
                });
                if huddle.is_live() && !in_call {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Join").on_hover_text("Join the huddle").clicked() {
                            join = true;
                        }
                    });
                }
            });
        });
    join
}

const ANSWER_LIGHT: egui::Color32 = egui::Color32::from_rgb(46, 182, 125);

fn card_text(huddle: &Huddle, names: Option<&[String]>) -> (String, String) {
    if huddle.is_live() {
        let count = huddle.participants.len();
        let detail = match names.filter(|names| names.len() == count) {
            Some([one]) => format!("{one} is in the huddle"),
            Some([one, two]) => format!("{one} and {two} are in the huddle"),
            Some([one, rest @ ..]) => format!("{one} and {} others are in the huddle", rest.len()),
            _ if count == 1 => "1 person is in the huddle".into(),
            _ => format!("{count} people are in the huddle"),
        };
        ("Huddle happening now".into(), detail)
    } else if huddle.ended && huddle.ended_at > huddle.started && huddle.started > 0 {
        let joined = huddle.joined_ever.len();
        let people = if joined == 1 {
            "1 person".into()
        } else {
            format!("{joined} people")
        };
        (
            "Huddle ended".into(),
            format!(
                "Lasted {} · {people}",
                duration(huddle.ended_at - huddle.started)
            ),
        )
    } else if huddle.ended {
        ("Huddle ended".into(), String::new())
    } else {
        ("Huddle".into(), "No one is in it right now".into())
    }
}

fn duration(seconds: i64) -> String {
    match seconds {
        ..60 => format!("{} s", seconds.max(1)),
        60..3600 => format!("{} min", seconds / 60),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

/// Headphones: a band over two ear cups.
pub fn paint_headphones(ui: &egui::Ui, rect: egui::Rect, color: egui::Color32) {
    let painter = ui.painter();
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.38;
    let stroke = egui::Stroke::new(1.6, color);
    let band: Vec<egui::Pos2> = (0..=16)
        .map(|step| {
            let angle = std::f32::consts::PI * (1.0 + step as f32 / 16.0);
            center + egui::vec2(angle.cos(), angle.sin()) * radius + egui::vec2(0.0, 1.0)
        })
        .collect();
    painter.add(egui::Shape::line(band, stroke));
    for side in [-1.0, 1.0] {
        let cup = egui::Rect::from_center_size(
            center + egui::vec2(side * radius, radius * 0.55),
            egui::vec2(radius * 0.55, radius * 0.95),
        );
        painter.rect_filled(cup, 1.5, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn huddle(participants: &[&str], ended: bool) -> Huddle {
        Huddle {
            id: "R1".into(),
            conversation_id: "D1".into(),
            participants: participants.iter().map(|id| id.to_string()).collect(),
            joined_ever: vec!["U1".into(), "U2".into()],
            link: Some("https://app.slack.com/huddle/T1/D1".into()),
            ended,
            started: 1_000,
            ended_at: if ended { 1_725 } else { 0 },
        }
    }

    #[test]
    fn ringing_stops_when_answered_elsewhere_or_the_huddle_ends() {
        let mut huddles = Huddles::default();
        huddles.ring("D1".into(), "U1".into(), "R1".into());
        assert!(
            huddles.update(huddle(&["U1"], false), "ME"),
            "names to look up"
        );
        assert!(
            huddles.ringing().is_some(),
            "still ringing while the caller waits"
        );
        huddles.update(huddle(&["U1", "ME"], false), "ME");
        assert!(huddles.ringing().is_none(), "answered on another device");

        huddles.ring("D1".into(), "U1".into(), "R1".into());
        assert!(
            huddles.stop_ringing_in(Some("D2")).is_none(),
            "another conversation"
        );
        assert!(huddles.ringing().is_some());
        huddles.update(huddle(&[], true), "ME");
        assert!(huddles.ringing().is_none());
        assert!(huddles.live("D1").is_none());
    }

    #[test]
    fn cards_name_who_is_in_and_how_long_it_lasted() {
        let live = huddle(&["U1", "U2", "U3"], false);
        assert_eq!(
            card_text(&live, Some(&["Lukas".into(), "Ada".into(), "Bo".into()])).1,
            "Lukas and 2 others are in the huddle"
        );
        assert_eq!(card_text(&live, None).1, "3 people are in the huddle");
        assert_eq!(
            card_text(&huddle(&[], true), None),
            ("Huddle ended".into(), "Lasted 12 min · 2 people".into())
        );
    }

    #[test]
    fn history_does_not_override_newer_live_state() {
        let mut huddles = Huddles::default();
        huddles.update(huddle(&[], true), "ME");
        huddles.seen(&huddle(&["U1"], false), "ME");
        assert!(
            huddles.live("D1").is_none(),
            "the same huddle, already known to have ended"
        );
        let mut later = huddle(&["U2"], false);
        later.id = "R2".into();
        later.started = 2_000;
        huddles.seen(&later, "ME");
        assert!(huddles.live("D1").is_some());
    }
}
