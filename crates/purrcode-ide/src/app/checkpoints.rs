//! Checkpoints: going back, and branching off.
//!
//! v1.2 Pillar 3. The runtime has had restorable checkpoints, worktree
//! rollback and session fork since the v1.2 backend landed; what was missing
//! was the simple mental model on top of them — "this step was wrong, go
//! back" — and a way to try a second approach without losing the first.
//!
//! Two rules shape this surface.
//!
//! A restore discards work. The dialog therefore states what will be lost
//! *before* asking, in real numbers fetched from the daemon's preview route,
//! and the confirm button stays disabled until those numbers have arrived.
//! Asking for blind consent and then deleting four files is exactly the
//! experience that teaches people not to trust an agent with their worktree.
//!
//! The conversation and the worktree are separate stores. Restoring one
//! without the other is a real operation, so the scope is an explicit choice
//! rather than an assumption.

use egui::{RichText, ScrollArea, Ui};

use super::PurrCodeIde;
use crate::daemon::Request;
use crate::model::{Checkpoint, RestoreScope};
use crate::theme;

/// A restore the user is being asked to confirm.
#[derive(Clone, Debug)]
pub(crate) struct PendingRestore {
    pub checkpoint: Checkpoint,
    pub scope: RestoreScope,
    /// The conversation message the restore is anchored to, when it was
    /// started from the transcript. Forking needs it; a restore started from
    /// the checkpoint list has none, which is why conversation scopes are not
    /// offered there.
    pub anchor_message_id: Option<String>,
}

impl PurrCodeIde {
    /// Fetches the checkpoint list for the selected session.
    pub(crate) fn refresh_checkpoints(&mut self) {
        let Some(session) = self.selected.clone() else {
            self.checkpoints.clear();
            self.checkpoints_for = None;
            return;
        };
        if self.checkpoints_for.as_deref() == Some(session.as_str()) {
            return;
        }
        self.checkpoints_for = Some(session.clone());
        self.client.send(Request::ListCheckpoints { session });
    }

    /// Opens the restore confirmation for a checkpoint.
    ///
    /// The preview is requested immediately; the dialog will not let the user
    /// confirm until it answers, so nobody agrees to discard work without
    /// being told how much.
    pub(crate) fn begin_restore(
        &mut self,
        checkpoint: Checkpoint,
        anchor_message_id: Option<String>,
    ) {
        let Some(session) = self.selected.clone() else {
            return;
        };
        let id = checkpoint.id.clone();
        self.pending_restore = Some(PendingRestore {
            checkpoint,
            // Both is the default because it is what "go back to here" means
            // to most people; the other two are there for when it does not.
            scope: RestoreScope::Both,
            anchor_message_id,
        });
        self.client.send(Request::CheckpointPreview {
            session,
            checkpoint: id,
        });
    }

    /// The checkpoint nearest at-or-before a conversation message.
    ///
    /// Checkpoints are stamped when a turn completes, so the one that
    /// describes the code as of a message is the newest one not created after
    /// it. Returns `None` when no checkpoint predates the message — the UI
    /// then offers only a conversation fork, because there is no code state
    /// to go back to.
    pub(crate) fn checkpoint_at(&self, message_timestamp: &str) -> Option<Checkpoint> {
        self.checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.created_at.as_str() <= message_timestamp)
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
            .cloned()
    }

    /// Draws the restore dialog.
    pub(crate) fn restore_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_restore.clone() else {
            return;
        };
        let tokens = self.tokens;
        let mut scope = pending.scope;
        // A restore started from the checkpoint list has no conversation
        // anchor, so the conversation scopes cannot be honoured there.
        let can_fork = pending.anchor_message_id.is_some();
        // Confirmation waits for the preview whenever code is at stake:
        // agreeing to discard an unknown amount of work is not consent.
        let ready = !scope.restores_code() || pending.checkpoint.changed_files.is_some();

        let (choice, ()) = super::primitives::dialog(
            ctx,
            &tokens,
            "purrcode_restore",
            "Restore",
            ("Restore", super::primitives::Tone::Danger),
            ready,
            |ui| {
                ui.label(
                    RichText::new(pending.checkpoint.display())
                        .size(theme::TYPE_META)
                        .color(tokens.text_secondary),
                );
                ui.add_space(10.0);

                for option in RestoreScope::ALL {
                    let allowed = can_fork || !option.forks_conversation();
                    ui.add_enabled_ui(allowed, |ui| {
                        ui.radio_value(&mut scope, *option, option.label())
                            .on_hover_text(if allowed {
                                option.description().to_owned()
                            } else {
                                "Start this from a message in the conversation to fork it."
                                    .to_owned()
                            });
                    });
                }
                ui.add_space(10.0);

                // What the operation costs, in real numbers. One axis, three
                // outcomes: the worktree is untouched, the cost is known, or
                // it is still being read.
                if !scope.restores_code() {
                    ui.label(
                        RichText::new("The worktree is left untouched.")
                            .size(theme::TYPE_META)
                            .color(tokens.text_secondary),
                    );
                    return;
                }
                let Some(files) = &pending.checkpoint.changed_files else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Checking what this would change…")
                                .size(theme::TYPE_META)
                                .color(tokens.text_muted),
                        );
                    });
                    return;
                };
                ui.label(
                    RichText::new(format!(
                        "Restoring rewrites {} file{} in this session's worktree.",
                        files.len(),
                        if files.len() == 1 { "" } else { "s" }
                    ))
                    .size(theme::TYPE_META)
                    .color(tokens.text_primary),
                );
                ui.add_space(2.0);
                ui.label(
                    RichText::new(
                        "Every change made after this checkpoint is discarded. This cannot be \
                         undone.",
                    )
                    .size(theme::TYPE_META)
                    .color(tokens.status_warning),
                );
                if files.is_empty() {
                    return;
                }
                ui.add_space(6.0);
                ScrollArea::vertical()
                    .id_salt("restore_files")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for file in files {
                            ui.label(
                                RichText::new(file)
                                    .monospace()
                                    .size(theme::TYPE_META)
                                    .color(tokens.text_muted),
                            );
                        }
                    });
            },
        );

        match choice {
            Some(super::primitives::DialogChoice::Confirm) => {
                self.commit_restore(&pending, scope);
                self.pending_restore = None;
            }
            Some(super::primitives::DialogChoice::Cancel) => self.pending_restore = None,
            None => {
                if let Some(pending) = self.pending_restore.as_mut() {
                    pending.scope = scope;
                }
            }
        }
    }

    fn commit_restore(&mut self, pending: &PendingRestore, scope: RestoreScope) {
        let Some(session) = self.selected.clone() else {
            return;
        };
        if scope.restores_code() {
            self.client.send(Request::RestoreCheckpoint {
                session: session.clone(),
                checkpoint: pending.checkpoint.id.clone(),
            });
        }
        if scope.forks_conversation()
            && let Some(anchor) = pending.anchor_message_id.clone()
        {
            self.client.send(Request::ForkSession {
                session,
                anchor_message_id: anchor,
            });
        }
    }

    /// The checkpoint list panel.
    pub(crate) fn checkpoints_panel(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        let mut capture = false;
        ui.horizontal(|ui| {
            self.section_heading(ui, "Checkpoints");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                capture |= self
                    .icon_action(ui, crate::icons::Glyph::Plus, "Capture a checkpoint now")
                    .clicked();
            });
        });
        if capture {
            self.capture_checkpoint();
        }

        if self.selected.is_none() {
            ui.label(
                RichText::new("Select a session to see its checkpoints.")
                    .small()
                    .color(tokens.text_muted),
            );
            return;
        }
        if self.checkpoints.is_empty() {
            ui.label(
                RichText::new(
                    "No checkpoints yet. One is captured after each agent turn, and you can \
                     capture one now.",
                )
                .small()
                .color(tokens.text_muted),
            );
            return;
        }

        // Moved out and put back rather than cloned: this is a draw path.
        let checkpoints = std::mem::take(&mut self.checkpoints);
        let mut restore: Option<Checkpoint> = None;
        ScrollArea::vertical()
            .id_salt("checkpoint_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for checkpoint in &checkpoints {
                    // `list_row` owns the identity/meta/action layout, so a
                    // narrow panel elides the label instead of running it
                    // under the button.
                    let asked = super::primitives::list_row(
                        ui,
                        &tokens,
                        super::primitives::RowSpec::new(&checkpoint.display())
                            .meta(&crate::model::relative_time(&checkpoint.created_at)),
                        |ui| {
                            super::primitives::button(
                                ui,
                                &tokens,
                                super::primitives::Tone::Secondary,
                                "Restore",
                            )
                            .clicked()
                        },
                    )
                    .inner;
                    if asked {
                        restore = Some(checkpoint.clone());
                    }
                }
            });
        self.checkpoints = checkpoints;
        if let Some(checkpoint) = restore {
            self.begin_restore(checkpoint, None);
        }
    }

    /// Captures a checkpoint of the session's worktree right now.
    pub(crate) fn capture_checkpoint(&mut self) {
        let Some(session) = self.selected.clone() else {
            return;
        };
        self.client.send(Request::CreateCheckpoint {
            session,
            label: "Manual checkpoint".to_owned(),
        });
    }

    /// Runs the action a transcript row asked for.
    pub(crate) fn apply_message_action(
        &mut self,
        action: MessageAction,
        message: &crate::model::Message,
    ) {
        match action {
            MessageAction::Restore => match self.checkpoint_at(&message.timestamp) {
                Some(checkpoint) => self.begin_restore(checkpoint, Some(message.id.clone())),
                // No checkpoint predates this message, so there is no code
                // state to return to. Say that rather than restoring the
                // nearest one and calling it "here".
                None => self.push_notice(crate::app::errors::Notice {
                    kind: crate::app::errors::NoticeKind::Environment,
                    headline: "No checkpoint covers this point".to_owned(),
                    impact: "PurrCode captures a checkpoint after each agent turn, and none was \
                             captured at or before this message."
                        .to_owned(),
                    next_step: Some("You can still fork the conversation from here.".to_owned()),
                    actions: vec![crate::app::errors::NoticeAction::Dismiss],
                    detail: None,
                    clears_on_reconnect: false,
                }),
            },
            MessageAction::Fork => {
                if let Some(session) = self.selected.clone() {
                    self.client.send(Request::ForkSession {
                        session,
                        anchor_message_id: message.id.clone(),
                    });
                }
            }
        }
    }
}

/// What a transcript row's hover controls asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MessageAction {
    Restore,
    Fork,
}

/// The per-message controls in the transcript: go back here, or branch.
///
/// A free function because the transcript draws while `self.session.messages`
/// is borrowed; the choice is returned and applied after the loop.
///
/// Nothing is drawn for a message with no id: the fork route anchors on that
/// id, so the control would fail on click. An affordance that silently does
/// nothing is worse than an absent one.
pub(crate) fn message_actions(
    ui: &mut Ui,
    tokens: &theme::Tokens,
    message: &crate::model::Message,
    hovered: bool,
) -> Option<MessageAction> {
    if message.id.is_empty() {
        return None;
    }
    // The controls stay out of the way until the row is under the pointer:
    // a transcript with two buttons under every message is a transcript
    // nobody can read.
    if !hovered {
        return None;
    }
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if ui
            .add(
                egui::Label::new(
                    RichText::new("↶ Restore here")
                        .size(theme::TYPE_EYEBROW)
                        .color(tokens.text_muted),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Go back to the code and conversation as they were here")
            .clicked()
        {
            chosen = Some(MessageAction::Restore);
        }
        if ui
            .add(
                egui::Label::new(
                    RichText::new("⑂ Fork from here")
                        .size(theme::TYPE_EYEBROW)
                        .color(tokens.text_muted),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Start a new session from this point, leaving this one intact")
            .clicked()
        {
            chosen = Some(MessageAction::Fork);
        }
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(id: &str, created_at: &str) -> Checkpoint {
        Checkpoint {
            id: id.into(),
            label: String::new(),
            head: "abcdef1234".into(),
            created_at: created_at.into(),
            changed_files: None,
        }
    }

    #[test]
    fn a_scope_states_exactly_which_stores_it_touches() {
        assert!(RestoreScope::Both.restores_code());
        assert!(RestoreScope::Both.forks_conversation());
        assert!(RestoreScope::CodeOnly.restores_code());
        assert!(!RestoreScope::CodeOnly.forks_conversation());
        assert!(!RestoreScope::ConversationOnly.restores_code());
        assert!(RestoreScope::ConversationOnly.forks_conversation());
    }

    #[test]
    fn a_checkpoint_with_no_label_is_named_by_its_head() {
        // "Checkpoint" alone in a list of five is useless; the head at least
        // distinguishes them.
        assert_eq!(
            checkpoint("c1", "2026-08-08T10:00:00Z").display(),
            "Checkpoint abcdef12"
        );
        let mut labelled = checkpoint("c1", "2026-08-08T10:00:00Z");
        labelled.label = "After the refactor".into();
        assert_eq!(labelled.display(), "After the refactor");
    }

    #[test]
    fn checkpoints_parse_from_the_daemon_list() {
        let parsed = Checkpoint::parse_all(&serde_json::json!([
            {"id": "c2", "label": "second", "head": "bbb", "created_at": "2026-08-08T11:00:00Z"},
            {"id": "c1", "label": "first", "head": "aaa", "created_at": "2026-08-08T10:00:00Z"},
        ]));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "c2");
        // The preview has not been fetched, which must stay distinguishable
        // from "this checkpoint changes nothing".
        assert!(parsed[0].changed_files.is_none());
    }

    #[test]
    fn a_malformed_checkpoint_entry_is_dropped_not_defaulted() {
        // An entry with no id cannot be restored, so offering it would be an
        // affordance that fails on click.
        let parsed = Checkpoint::parse_all(&serde_json::json!([
            {"label": "no id here"},
            {"id": "c1", "head": "aaa", "created_at": "2026-08-08T10:00:00Z"},
        ]));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "c1");
    }
}
