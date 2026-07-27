//! Keyboard dispatch for the TUI.

use crate::app::{App, AppMode};
use crate::provider_setup::{ProviderSetup, SetupScreen};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    if is_active_pull_cancel_key(key, app.active_pull_action.is_some()) {
        app.pending_command = Some("/model pull-cancel".into());
        return true;
    }
    if is_active_stream_cancel_key(key, app.mode == AppMode::Conversation && app.stream.active) {
        app.pending_command = Some("/cancel".into());
        return true;
    }
    match app.mode {
        AppMode::SecretReview => handle_secret_review_key(app, key),
        AppMode::ProviderSetup => handle_setup_key(app, key),
        AppMode::SkillBrowse => handle_skill_key(app, key),
        AppMode::DiffView => handle_diff_key(app, key),
        AppMode::Help => handle_help_key(app, key),
        AppMode::LeaseConflict => handle_lease_conflict_key(app, key),
        AppMode::Conversation => handle_conversation_key(app, key),
    }
}

fn handle_help_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.palette_query.clear();
            app.palette_selected = 0;
            app.switch_mode(AppMode::Conversation);
        }
        KeyCode::Up => app.palette_selected = app.palette_selected.saturating_sub(1),
        KeyCode::Down => {
            let count = crate::command_palette::filtered_actions(&app.palette_query).len();
            app.palette_selected = app
                .palette_selected
                .saturating_add(1)
                .min(count.saturating_sub(1));
        }
        KeyCode::Backspace => {
            app.palette_query.pop();
            app.palette_selected = 0;
        }
        KeyCode::Char(character) => {
            app.palette_query.push(character);
            app.palette_selected = 0;
        }
        KeyCode::Enter => {
            if let Some(action) = crate::command_palette::filtered_actions(&app.palette_query)
                .get(app.palette_selected)
            {
                app.pending_command = Some(action.2.to_owned());
                app.palette_query.clear();
                app.palette_selected = 0;
                app.switch_mode(AppMode::Conversation);
            }
        }
        _ => {}
    }
    true
}

fn handle_conversation_key(app: &mut App, key: KeyEvent) -> bool {
    if let Some(command) = model_pull_shortcut(
        key,
        app.composer.buffer.is_empty(),
        app.pending_model_pull.is_some(),
    ) {
        app.pending_command = Some(command.into());
        return true;
    }

    if is_submit_key(key) {
        if let Ok(detection) = purrcode_provider_import::detect_content(&app.composer.buffer) {
            if !detection.secret_findings.is_empty() {
                let redacted = purrcode_provider_import::redact_source(&app.composer.buffer)
                    .expect("content was already size-checked");
                app.secret_review = Some(crate::app::SecretReview {
                    redacted_source: redacted.display,
                    finding_count: redacted.findings.len(),
                    provider_candidate: detection.kind
                        == purrcode_provider_import::ContentKind::ProviderConfiguration,
                });
                app.switch_mode(AppMode::SecretReview);
                return true;
            }
        }
        let msg = app.composer.submit();
        if msg.starts_with('/') && !msg.contains('\n') {
            app.pending_command = Some(msg);
        } else if !msg.is_empty() {
            app.conversation.add_user_message(&msg);
            app.pending_user_message = true;
        }
        return true;
    }

    match key.code {
        KeyCode::Char('q') if app.composer.buffer.is_empty() => return false,
        KeyCode::Char('a')
            if app.composer.buffer.is_empty() && app.conversation.pending_action.is_some() =>
        {
            app.pending_command = Some("/approve".into())
        }
        KeyCode::Char('r')
            if app.composer.buffer.is_empty() && app.conversation.pending_action.is_some() =>
        {
            app.pending_command = Some("/deny rejected from approval card".into())
        }
        KeyCode::Char('/') if app.composer.buffer.is_empty() => {
            app.composer.buffer.push('/');
            app.composer.cursor = 1;
        }
        KeyCode::Enter => app.composer.insert_newline(),
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.select_all()
        }
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.workspace.toggle_files()
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.pending_command = Some("/diff".into())
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.conversation.select_card(-1)
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.conversation.select_card(1)
        }
        KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.conversation.toggle_selected_card()
        }
        KeyCode::Char('?') if app.composer.buffer.is_empty() => app.switch_mode(AppMode::Help),
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.switch_mode(AppMode::Help)
        }
        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => app.composer.undo(),
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => app.composer.redo(),
        KeyCode::Char(c) => app.composer.insert_char(c),
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.delete_word_before()
        }
        KeyCode::Delete if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.delete_word_after()
        }
        KeyCode::Backspace => app.composer.delete_before(),
        KeyCode::Delete => app.composer.delete_after(),
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.move_word_left()
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.move_word_right()
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.composer.select_move_left()
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.composer.select_move_right()
        }
        KeyCode::Left => app.composer.move_left(),
        KeyCode::Right => app.composer.move_right(),
        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.move_document_start()
        }
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.move_document_end()
        }
        KeyCode::Home => app.composer.move_home(),
        KeyCode::End => app.composer.move_end(),
        KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => app.composer.history_up(),
        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => app.composer.history_down(),
        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.composer.select_move_vertical(-1)
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.composer.select_move_vertical(1)
        }
        KeyCode::PageUp => app.composer.move_page(-1),
        KeyCode::PageDown => app.composer.move_page(1),
        KeyCode::Up => app.composer.move_up(),
        KeyCode::Down => app.composer.move_down(),
        KeyCode::Esc => {
            if app.composer.buffer.is_empty() {
                return false;
            }
            app.composer = crate::composer::Composer::new();
        }
        KeyCode::BackTab => app.composer.outdent_current_line(),
        KeyCode::Tab => app.composer.insert_tab(),
        _ => {}
    }
    true
}

fn model_pull_shortcut(
    key: KeyEvent,
    composer_empty: bool,
    has_pending_pull: bool,
) -> Option<&'static str> {
    if matches!(key.code, KeyCode::Char('p' | 'P'))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        && composer_empty
        && has_pending_pull
    {
        return Some("/model pull-approve");
    }
    None
}

fn is_active_pull_cancel_key(key: KeyEvent, has_active_pull: bool) -> bool {
    matches!(key.code, KeyCode::Char('c' | 'C'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && has_active_pull
}

fn is_active_stream_cancel_key(key: KeyEvent, has_active_stream: bool) -> bool {
    matches!(key.code, KeyCode::Char('c' | 'C'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && has_active_stream
}

fn handle_lease_conflict_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('r') | KeyCode::Char('R') => {
            app.pending_command = Some("/resume".into());
            app.switch_mode(AppMode::Conversation);
        }
        KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Esc => {
            app.switch_mode(AppMode::Conversation);
            app.message_bar = "Attached read-only; draft preserved.".into();
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.session_id = None;
            app.switch_mode(AppMode::Conversation);
            app.message_bar = "New session selected; draft preserved.".into();
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            app.message_bar =
                "Daemon returned HTTP 409 Conflict while acquiring the session lease.".into();
        }
        _ => {}
    }
    true
}

fn handle_secret_review_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('r') | KeyCode::Char('R') => {
            let Some(review) = app.secret_review.take() else {
                return true;
            };
            app.composer.replace_sensitive_with(review.redacted_source);
            let message = app.composer.submit();
            app.conversation.add_user_message(&message);
            app.pending_user_message = true;
            app.switch_mode(AppMode::Conversation);
            app.message_bar = "Secret-like values were redacted before sending.".into();
        }
        KeyCode::Char('i') | KeyCode::Char('I') => {
            let source = std::mem::take(&mut app.composer.buffer);
            app.composer.replace_sensitive_with(String::new());
            let mut setup = ProviderSetup::import_mode();
            setup.import_source = source;
            setup.review_import();
            app.secret_review = None;
            app.provider_setup = Some(setup);
            app.switch_mode(AppMode::ProviderSetup);
            app.message_bar = "Review imported provider fields before testing.".into();
        }
        KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
            app.secret_review = None;
            app.switch_mode(AppMode::Conversation);
            app.message_bar = "Send cancelled; draft preserved.".into();
        }
        _ => {}
    }
    true
}

fn handle_setup_key(app: &mut App, key: KeyEvent) -> bool {
    if is_submit_key(key) {
        if let Some(ref mut setup) = app.provider_setup {
            match setup.screen {
                SetupScreen::ImportSource => setup.review_import(),
                SetupScreen::ImportAuthChoice => setup.choose_import_auth(),
                SetupScreen::ImportEnvironment => setup.confirm_environment_reference(),
                SetupScreen::ImportKeychainConfirm => {}
                SetupScreen::Discovery | SetupScreen::Form | SetupScreen::ImportReview => {
                    setup.request_test_and_save()
                }
            }
        }
        return true;
    }

    match key.code {
        KeyCode::Esc => {
            let nested = app.provider_setup.as_ref().map(|setup| setup.screen);
            match nested {
                Some(SetupScreen::ImportKeychainConfirm | SetupScreen::ImportEnvironment) => {
                    if let Some(setup) = &mut app.provider_setup {
                        setup.screen = SetupScreen::ImportAuthChoice;
                        setup.error = None;
                    }
                }
                _ => {
                    app.provider_setup = None;
                    app.switch_mode(AppMode::Conversation);
                }
            }
        }
        KeyCode::Enter => {
            if let Some(setup) = &mut app.provider_setup {
                match setup.screen {
                    SetupScreen::Discovery => setup.choose_selected(),
                    SetupScreen::ImportSource => setup.insert_import("\n"),
                    SetupScreen::ImportAuthChoice => setup.choose_import_auth(),
                    SetupScreen::ImportEnvironment => setup.confirm_environment_reference(),
                    SetupScreen::ImportKeychainConfirm => {}
                    SetupScreen::Form | SetupScreen::ImportReview => setup.next_field(false),
                }
            }
        }
        KeyCode::Up
            if app.provider_setup.as_ref().is_some_and(|setup| {
                matches!(
                    setup.screen,
                    SetupScreen::Discovery | SetupScreen::ImportAuthChoice
                )
            }) =>
        {
            if let Some(setup) = &mut app.provider_setup {
                if setup.screen == SetupScreen::Discovery {
                    setup.move_selection(-1);
                } else {
                    setup.move_import_auth_choice(-1);
                }
            }
        }
        KeyCode::Down
            if app.provider_setup.as_ref().is_some_and(|setup| {
                matches!(
                    setup.screen,
                    SetupScreen::Discovery | SetupScreen::ImportAuthChoice
                )
            }) =>
        {
            if let Some(setup) = &mut app.provider_setup {
                if setup.screen == SetupScreen::Discovery {
                    setup.move_selection(1);
                } else {
                    setup.move_import_auth_choice(1);
                }
            }
        }
        KeyCode::Tab => {
            if let Some(setup) = &mut app.provider_setup {
                if matches!(setup.screen, SetupScreen::Form | SetupScreen::ImportReview) {
                    setup.next_field(false);
                }
            }
        }
        KeyCode::BackTab => {
            if let Some(setup) = &mut app.provider_setup {
                if matches!(setup.screen, SetupScreen::Form | SetupScreen::ImportReview) {
                    setup.next_field(true);
                }
            }
        }
        KeyCode::Char('y' | 'Y')
            if app
                .provider_setup
                .as_ref()
                .is_some_and(|setup| setup.screen == SetupScreen::ImportKeychainConfirm) =>
        {
            if let Some(setup) = &mut app.provider_setup {
                setup.confirm_keychain_choice(true);
            }
        }
        KeyCode::Char('n' | 'N')
            if app
                .provider_setup
                .as_ref()
                .is_some_and(|setup| setup.screen == SetupScreen::ImportKeychainConfirm) =>
        {
            if let Some(setup) = &mut app.provider_setup {
                setup.confirm_keychain_choice(false);
            }
        }
        KeyCode::Char('k' | 'K')
            if app
                .provider_setup
                .as_ref()
                .is_some_and(|setup| setup.screen == SetupScreen::ImportAuthChoice) =>
        {
            if let Some(setup) = &mut app.provider_setup {
                setup.import_auth_choice = 0;
                setup.choose_import_auth();
            }
        }
        KeyCode::Char('e' | 'E')
            if app
                .provider_setup
                .as_ref()
                .is_some_and(|setup| setup.screen == SetupScreen::ImportAuthChoice) =>
        {
            if let Some(setup) = &mut app.provider_setup {
                setup.import_auth_choice = 1;
                setup.choose_import_auth();
            }
        }
        KeyCode::Char('d' | 'D')
            if app
                .provider_setup
                .as_ref()
                .is_some_and(|setup| setup.screen == SetupScreen::ImportAuthChoice) =>
        {
            if let Some(setup) = &mut app.provider_setup {
                setup.import_auth_choice = 2;
                setup.choose_import_auth();
            }
        }
        KeyCode::Char(character) => {
            if let Some(setup) = &mut app.provider_setup {
                match setup.screen {
                    SetupScreen::ImportSource => setup.insert_import(&character.to_string()),
                    SetupScreen::ImportEnvironment => setup.edit_environment_reference(character),
                    SetupScreen::Form | SetupScreen::ImportReview => setup.edit_char(character),
                    SetupScreen::Discovery
                    | SetupScreen::ImportAuthChoice
                    | SetupScreen::ImportKeychainConfirm => {}
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(setup) = &mut app.provider_setup {
                match setup.screen {
                    SetupScreen::ImportSource => {
                        setup.import_source.pop();
                    }
                    SetupScreen::ImportEnvironment => {
                        setup.backspace_environment_reference();
                    }
                    SetupScreen::Form | SetupScreen::ImportReview => setup.backspace(),
                    SetupScreen::Discovery
                    | SetupScreen::ImportAuthChoice
                    | SetupScreen::ImportKeychainConfirm => {}
                }
            }
        }
        _ => {}
    }
    true
}

/// Return whether a key should submit the current multiline input.
///
/// Many terminals, including Apple Terminal, encode Ctrl+Enter exactly like Enter and therefore
/// cannot report the Control modifier. Ctrl+G has a distinct control code in traditional and
/// enhanced terminal protocols, so it is the portable send binding. Modified Enter remains
/// supported when the terminal can report it.
fn is_submit_key(key: KeyEvent) -> bool {
    (key.code == KeyCode::Enter
        && key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT))
        || (key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL))
}

fn handle_skill_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.skill_browser = None;
            app.switch_mode(AppMode::Conversation);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(ref mut browser) = app.skill_browser {
                browser.selected = browser
                    .selected
                    .saturating_add(1)
                    .min(browser.skills.len().saturating_sub(1));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(ref mut browser) = app.skill_browser {
                browser.selected = browser.selected.saturating_sub(1);
            }
        }
        _ => {}
    }
    true
}

fn handle_diff_key(app: &mut App, _key: KeyEvent) -> bool {
    app.diff_view = None;
    app.switch_mode(AppMode::Conversation);
    true
}

#[cfg(test)]
mod tests {
    use super::{
        is_active_pull_cancel_key, is_active_stream_cancel_key, is_submit_key, model_pull_shortcut,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn portable_control_g_submits() {
        assert!(is_submit_key(KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn modified_enter_submits_when_terminal_reports_modifier() {
        for modifier in [
            KeyModifiers::CONTROL,
            KeyModifiers::SUPER,
            KeyModifiers::ALT,
        ] {
            assert!(is_submit_key(KeyEvent::new(KeyCode::Enter, modifier)));
        }
    }

    #[test]
    fn plain_enter_remains_a_newline() {
        assert!(!is_submit_key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
    }

    #[test]
    fn p_only_approves_a_visible_pending_model_pull() {
        let p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(
            model_pull_shortcut(p, true, true),
            Some("/model pull-approve")
        );
        assert_eq!(model_pull_shortcut(p, false, true), None);
        assert_eq!(model_pull_shortcut(p, true, false), None);
    }

    #[test]
    fn control_c_cancels_only_an_active_model_pull() {
        let control_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(is_active_pull_cancel_key(control_c, true));
        assert!(!is_active_pull_cancel_key(control_c, false));
    }

    #[test]
    fn control_c_cancels_only_an_active_session_stream() {
        let control_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(is_active_stream_cancel_key(control_c, true));
        assert!(!is_active_stream_cancel_key(control_c, false));
    }
}
