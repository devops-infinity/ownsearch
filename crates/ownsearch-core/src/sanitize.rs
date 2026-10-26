fn is_invisible(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{180B}'..='\u{180F}'
            | '\u{034F}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{E0000}'..='\u{E007F}'
            | '\u{E0100}'..='\u{E01EF}'
            | '\u{0600}'..='\u{0605}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{E0080}'..='\u{E00FF}'
            | '\u{E01F0}'..='\u{E0FFF}'
    ) || matches!(
        character,
        '\u{061C}' | '\u{FFA0}' | '\u{180E}' | '\u{FFF9}'..='\u{FFFB}' | '\u{1D173}'..='\u{1D17A}'
    )
}

fn is_break(character: char) -> bool {
    matches!(
        character,
        '\r' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for character in text.chars() {
        if is_invisible(character) {
            continue;
        }
        let character = if character == '\n' || character == '\t' || is_break(character) {
            ' '
        } else if character.is_control() {
            continue;
        } else {
            character
        };
        if character == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(character);
    }
    let trimmed = out.trim();
    if trimmed.len() == out.len() {
        out
    } else {
        trimmed.to_owned()
    }
}

pub fn cap(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_owned(), false);
    }
    let mut capped: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    capped.push('\u{2026}');
    (capped, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invisible_characters_are_removed() {
        let dirty = "hel\u{200B}lo\u{FEFF} wor\u{00AD}ld";
        assert_eq!(sanitize(dirty), "hello world");
    }

    #[test]
    fn control_characters_never_survive() {
        let dirty = "a\u{0}b\u{7}c\u{1b}d";
        let clean = sanitize(dirty);
        assert!(clean.chars().all(|character| !character.is_control()));
    }

    #[test]
    fn runs_of_whitespace_collapse_to_one_space() {
        assert_eq!(sanitize("a \t\n  b"), "a b");
    }

    #[test]
    fn a_long_value_is_capped_with_a_visible_marker() {
        let (capped, cut) = cap("abcdefghij", 5);
        assert!(cut);
        assert_eq!(capped, "abcd\u{2026}");
        assert_eq!(capped.chars().count(), 5);
    }
}
