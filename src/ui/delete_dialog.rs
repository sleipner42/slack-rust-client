use eframe::egui;

use super::theme;

/// What the user asked to delete.
#[derive(Clone, Debug, PartialEq)]
pub enum Removal {
    /// A message, and the IDs of the files shared in it.
    Message {
        ts: String,
        files: Vec<String>,
    },
    File {
        id: String,
        name: String,
    },
}

pub enum DeleteOutcome {
    Open,
    /// Delete what the dialog shows; see [`DeleteDialog::file_ids`].
    Confirm,
    Closed,
}

/// Asks before deleting, since Slack cannot bring a message or file back.
pub struct DeleteDialog {
    pub conversation_id: String,
    pub removal: Removal,
    /// Delete a message's files along with it.
    pub with_files: bool,
    pub busy: bool,
    pub error: Option<String>,
}

impl DeleteDialog {
    pub fn new(conversation_id: String, removal: Removal) -> Self {
        Self {
            conversation_id,
            removal,
            with_files: true,
            busy: false,
            error: None,
        }
    }

    /// The files that will be deleted.
    pub fn file_ids(&self) -> Vec<String> {
        match &self.removal {
            Removal::Message { files, .. } if self.with_files => files.clone(),
            Removal::Message { .. } => Vec::new(),
            Removal::File { id, .. } => vec![id.clone()],
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> DeleteOutcome {
        let (escape, enter) = ctx.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        if escape && !self.busy {
            return DeleteOutcome::Closed;
        }
        let (title, body) = match &self.removal {
            Removal::Message { .. } => (
                "Delete message",
                "Are you sure you want to delete this message? This cannot be undone.".to_string(),
            ),
            Removal::File { name, .. } => (
                "Delete file",
                format!(
                    "Are you sure you want to delete {name}? It is removed everywhere it was shared, and this cannot be undone."
                ),
            ),
        };
        let mut confirm = enter;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("delete_dialog"))
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(MODAL_WIDTH);
                ui.heading(title);
                ui.add_space(8.0);
                ui.label(body);
                if let Removal::Message { files, .. } = &self.removal
                    && !files.is_empty()
                {
                    ui.add_space(8.0);
                    let label = match files.len() {
                        1 => "Also delete the file in it".to_string(),
                        n => format!("Also delete the {n} files in it"),
                    };
                    ui.add_enabled(!self.busy, egui::Checkbox::new(&mut self.with_files, label));
                }
                if let Some(error) = &self.error {
                    ui.add_space(8.0);
                    ui.colored_label(theme::UNREAD_DOT, error);
                }
                ui.add_space(16.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.busy {
                        ui.spinner();
                    } else {
                        confirm |= ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Delete").color(egui::Color32::WHITE),
                                )
                                .fill(theme::DANGER),
                            )
                            .on_hover_text("Enter")
                            .clicked();
                        cancel = ui.button("Cancel").on_hover_text("Escape").clicked();
                    }
                });
            });
        if self.busy {
            DeleteOutcome::Open
        } else if cancel {
            DeleteOutcome::Closed
        } else if confirm {
            DeleteOutcome::Confirm
        } else {
            DeleteOutcome::Open
        }
    }
}

const MODAL_WIDTH: f32 = 400.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_takes_its_files_only_when_asked() {
        let mut dialog = DeleteDialog::new(
            "C1".into(),
            Removal::Message {
                ts: "1.0".into(),
                files: vec!["F1".into(), "F2".into()],
            },
        );
        assert_eq!(dialog.file_ids(), ["F1", "F2"]);
        dialog.with_files = false;
        assert!(dialog.file_ids().is_empty());
        let file = DeleteDialog::new(
            "C1".into(),
            Removal::File {
                id: "F3".into(),
                name: "a.png".into(),
            },
        );
        assert_eq!(file.file_ids(), ["F3"]);
    }
}
