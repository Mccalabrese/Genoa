use zeroize::{Zeroize, Zeroizing};

/// A bounded password buffer. It never exposes its contents for display.
#[derive(Default)]
pub struct Password {
    value: Zeroizing<String>,
}

impl Password {
    // Linux-PAM accepts passwords up to 512 bytes in common configurations.
    // A larger cap leaves room for unusual stacks while preventing input-driven
    // memory growth if a compositor sends a malicious stream of key events.
    const MAX_BYTES: usize = 1024;

    pub fn push_text(&mut self, text: &str) {
        let remaining = Self::MAX_BYTES.saturating_sub(self.value.len());
        if remaining == 0 {
            return;
        }

        let mut accepted = String::new();
        for character in text.chars().filter(|character| !character.is_control()) {
            if accepted.len() + character.len_utf8() > remaining {
                break;
            }
            accepted.push(character);
        }
        self.value.push_str(&accepted);
        accepted.zeroize();
    }

    pub fn backspace(&mut self) {
        let _ = self.value.pop();
    }

    pub fn take(&mut self) -> Zeroizing<String> {
        std::mem::take(&mut self.value)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.value.len()
    }
}

#[cfg(test)]
mod tests {
    use super::Password;

    #[test]
    fn rejects_control_characters() {
        let mut password = Password::default();
        password.push_text("a\n\u{1b}b");
        assert_eq!(password.take().as_str(), "ab");
    }

    #[test]
    fn respects_utf8_boundaries_at_the_limit() {
        let mut password = Password::default();
        password.push_text(&"a".repeat(1023));
        password.push_text("é");
        assert_eq!(password.len(), 1023);
    }

    #[test]
    fn backspace_removes_one_unicode_scalar() {
        let mut password = Password::default();
        password.push_text("aé");
        password.backspace();
        assert_eq!(password.take().as_str(), "a");
    }
}
