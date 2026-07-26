//! Keyboard dispatch for the TUI.

use crate::app::{App, AppMode};
use crate::provider_setup::ProviderType;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    match app.mode {
        AppMode::ProviderSetup => handle_setup_key(app, key),
        AppMode::SkillBrowse => handle_skill_key(app, key),
        AppMode::DiffView => handle_diff_key(app, key),
        AppMode::Conversation => handle_conversation_key(app, key),
    }
}

fn handle_conversation_key(app: &mut App, key: KeyEvent) -> bool {
    let submit = key.code == KeyCode::Enter
        && key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT);
    if submit {
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
        KeyCode::Char('/') if app.composer.buffer.is_empty() => {
            app.composer.buffer.push('/');
            app.composer.cursor = 1;
        }
        KeyCode::Enter => app.composer.insert_newline(),
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.select_all()
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

fn handle_setup_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.provider_setup = None;
            app.switch_mode(AppMode::Conversation);
        }
        KeyCode::Enter => {
            if let Some(ref mut setup) = app.provider_setup {
                if !setup.complete {
                    setup.advance();
                }
            }
        }
        KeyCode::Char(choice @ '1'..='5')
            if app
                .provider_setup
                .as_ref()
                .is_some_and(|s| s.provider_type.is_none()) =>
        {
            let provider = match choice {
                '1' => ProviderType::Ollama,
                '2' => ProviderType::LmStudio,
                '3' => ProviderType::Openai,
                '4' => ProviderType::OpenaiCompatible,
                _ => ProviderType::EnterpriseGateway,
            };
            if let Some(setup) = &mut app.provider_setup {
                setup.select_provider(provider);
            }
        }
        KeyCode::Char(character) => {
            if let Some(setup) = &mut app.provider_setup {
                if setup.provider_type == Some(ProviderType::Openai) && setup.step == 0 {
                    setup.api_key.push(character);
                } else {
                    setup.model_id.push(character);
                }
                setup.error = None;
            }
        }
        KeyCode::Backspace => {
            if let Some(setup) = &mut app.provider_setup {
                if setup.provider_type == Some(ProviderType::Openai) && setup.step == 0 {
                    setup.api_key.pop();
                } else {
                    setup.model_id.pop();
                }
            }
        }
        _ => {}
    }
    true
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
