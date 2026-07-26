//! Secure hidden credential input with zeroize semantics.

use std::io;

pub struct CredentialPrompt {
    pub label: String,
    pub buffer: String,
    pub complete: bool,
    pub storage_choice: StorageChoice,
    pub step: usize,
}

pub enum StorageChoice {
    Keychain,
    Session,
    Environment,
}

impl CredentialPrompt {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            buffer: String::new(),
            complete: false,
            storage_choice: StorageChoice::Keychain,
            step: 0,
        }
    }

    pub fn read_hidden(&mut self) -> io::Result<String> {
        crossterm::terminal::disable_raw_mode()?;
        let value = rpassword::prompt_password(format!("{}: ", self.label))?;
        crossterm::terminal::enable_raw_mode()?;
        Ok(value)
    }

    pub fn zeroize(&mut self) {
        self.buffer.clear();
        self.buffer.shrink_to(0);
    }
}
