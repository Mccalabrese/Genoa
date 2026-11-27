use zeroize::{Zeroize, Zeroizing};

use crate::protocol::MAX_PASSWORD_BYTES;

/// A bounded password buffer. It never exposes its contents for display.
pub struct Password {
    value: Zeroizing<String>,
}

impl Default for Password {
    fn default() -> Self {
        Self {
            value: Zeroizing::new(String::with_capacity(MAX_PASSWORD_BYTES)),
        }
    }
}

impl Password {
    // Linux-PAM accepts passwords up to 512 bytes in common configurations.
    // A larger cap leaves room for unusual stacks while preventing input-driven
    // memory growth if a compositor sends a malicious stream of key events.
    pub fn push_text(&mut self, text: &str) {
        for character in text.chars().filter(|character| !character.is_control()) {
            if self.value.len() + character.len_utf8() > MAX_PASSWORD_BYTES {
                break;
            }
            self.value.push(character);
        }
    }

    pub fn backspace(&mut self) {
        let _ = self.value.pop();
    }

    pub fn clear(&mut self) {
        self.value.zeroize();
    }

    pub fn take(&mut self) -> Zeroizing<String> {
        std::mem::replace(
            &mut self.value,
            Zeroizing::new(String::with_capacity(MAX_PASSWORD_BYTES)),
        )
    }

    pub fn display_len(&self) -> usize {
        self.value.chars().count()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.value.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_PASSWORD_BYTES, Password};

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

    #[test]
    fn clear_removes_entered_text() {
        let mut password = Password::default();
        password.push_text("secret");
        password.clear();
        assert_eq!(password.display_len(), 0);
    }

    #[test]
    fn preallocates_the_bounded_input_capacity() {
        assert!(Password::default().value.capacity() >= MAX_PASSWORD_BYTES);
    }

    #[test]
    fn take_replaces_the_password_with_a_preallocated_buffer() {
        let mut password = Password::default();
        password.push_text("secret");
        let submitted = password.take();

        assert_eq!(submitted.as_str(), "secret");
        assert!(password.value.capacity() >= MAX_PASSWORD_BYTES);
    }
}
