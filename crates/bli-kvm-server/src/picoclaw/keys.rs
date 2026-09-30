//! Key names and characters for the assistant's HID actions, as the
//! reference accepts them (browser `KeyboardEvent.code` names, aliases and a
//! US layout for typed text).

const LEFT_SHIFT: u8 = 1 << 1;

fn modifier(name: &str) -> Option<u8> {
    Some(match name {
        "ControlLeft" => 1 << 0,
        "ShiftLeft" => 1 << 1,
        "AltLeft" => 1 << 2,
        "MetaLeft" => 1 << 3,
        "ControlRight" => 1 << 4,
        "ShiftRight" => 1 << 5,
        "AltRight" => 1 << 6,
        "MetaRight" => 1 << 7,
        _ => return None,
    })
}

fn alias(upper: &str) -> Option<&'static str> {
    Some(match upper {
        "CTRL" | "CONTROL" => "ControlLeft",
        "SHIFT" => "ShiftLeft",
        "ALT" | "OPTION" => "AltLeft",
        "META" | "WIN" | "WINDOWS" | "COMMAND" | "CMD" | "SUPER" => "MetaLeft",
        "ESC" => "Escape",
        "RETURN" | "ENTER" => "Enter",
        "DEL" => "Delete",
        "INS" => "Insert",
        "UP" => "ArrowUp",
        "DOWN" => "ArrowDown",
        "LEFT" => "ArrowLeft",
        "RIGHT" => "ArrowRight",
        "SPACEBAR" | "SPACE" => "Space",
        "TAB" => "Tab",
        "BACKSPACE" => "Backspace",
        "PGUP" => "PageUp",
        "PGDN" => "PageDown",
        _ => return None,
    })
}

fn keycode(name: &str) -> Option<u8> {
    if let Some(letter) = name.strip_prefix("Key")
        && let [byte @ b'A'..=b'Z'] = letter.as_bytes()
    {
        return Some(0x04 + (byte - b'A'));
    }
    if let Some(digit) = name.strip_prefix("Digit")
        && let [byte @ b'0'..=b'9'] = digit.as_bytes()
    {
        return Some(if *byte == b'0' {
            0x27
        } else {
            0x1e + (byte - b'1')
        });
    }
    if let Some(number) = name.strip_prefix('F')
        && let Ok(number @ 1..=24) = number.parse::<u8>()
    {
        return Some(if number <= 12 {
            0x3a + number - 1
        } else {
            0x68 + number - 13
        });
    }
    if let Some(digit) = name.strip_prefix("Numpad")
        && let [byte @ b'0'..=b'9'] = digit.as_bytes()
    {
        return Some(if *byte == b'0' {
            0x62
        } else {
            0x59 + (byte - b'1')
        });
    }
    Some(match name {
        "Enter" => 0x28,
        "Escape" => 0x29,
        "Backspace" => 0x2a,
        "Tab" => 0x2b,
        "Space" => 0x2c,
        "Minus" => 0x2d,
        "Equal" => 0x2e,
        "BracketLeft" => 0x2f,
        "BracketRight" => 0x30,
        "Backslash" => 0x31,
        "IntlHash" => 0x32,
        "Semicolon" => 0x33,
        "Quote" => 0x34,
        "Backquote" => 0x35,
        "Comma" => 0x36,
        "Period" => 0x37,
        "Slash" => 0x38,
        "CapsLock" => 0x39,
        "PrintScreen" => 0x46,
        "ScrollLock" => 0x47,
        "Pause" => 0x48,
        "Insert" => 0x49,
        "Home" => 0x4a,
        "PageUp" => 0x4b,
        "Delete" => 0x4c,
        "End" => 0x4d,
        "PageDown" => 0x4e,
        "ArrowRight" => 0x4f,
        "ArrowLeft" => 0x50,
        "ArrowDown" => 0x51,
        "ArrowUp" => 0x52,
        "NumLock" => 0x53,
        "NumpadDivide" => 0x54,
        "NumpadMultiply" => 0x55,
        "NumpadSubtract" => 0x56,
        "NumpadAdd" => 0x57,
        "NumpadEnter" => 0x58,
        "NumpadDecimal" => 0x63,
        "IntlBackslash" => 0x64,
        "ContextMenu" => 0x65,
        "Power" => 0x66,
        "NumpadEqual" => 0x67,
        "AudioVolumeMute" | "VolumeMute" => 0x7f,
        "AudioVolumeUp" | "VolumeUp" => 0x80,
        "AudioVolumeDown" | "VolumeDown" => 0x81,
        _ => return None,
    })
}

/// A key name, alias, letter or digit: `(bits, true)` for a modifier,
/// `(usage, false)` otherwise.
pub(crate) fn resolve(key: &str) -> Result<(u8, bool), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("empty key".into());
    }
    if let Some(bits) = modifier(trimmed) {
        return Ok((bits, true));
    }
    if let Some(code) = keycode(trimmed) {
        return Ok((code, false));
    }
    let upper = trimmed.to_ascii_uppercase();
    if let Some(name) = alias(&upper) {
        return Ok(match modifier(name) {
            Some(bits) => (bits, true),
            None => (keycode(name).expect("aliases name known keys"), false),
        });
    }
    if let [byte] = upper.as_bytes() {
        if byte.is_ascii_uppercase() {
            return Ok((0x04 + (byte - b'A'), false));
        }
        if byte.is_ascii_digit() {
            return Ok((
                keycode(&format!("Digit{}", *byte as char)).expect("digit"),
                false,
            ));
        }
    }
    Err(format!("unsupported key: {key}"))
}

/// The keyboard report for a hotkey: modifiers plus up to six keys.
pub(crate) fn hotkey_report(keys: &[String]) -> Result<[u8; 8], String> {
    if keys.is_empty() {
        return Err("hotkey requires at least one key".into());
    }
    let mut report = [0_u8; 8];
    let mut next = 2;
    for key in keys {
        let (code, is_modifier) = resolve(key)?;
        if is_modifier {
            report[0] |= code;
        } else if next < report.len() {
            report[next] = code;
            next += 1;
        } else {
            return Err("hotkey supports at most 6 non-modifier keys".into());
        }
    }
    Ok(report)
}

/// The US-layout key for one typed character.
pub(crate) fn character(ch: char) -> Option<(u8, u8)> {
    let shifted = |code| Some((LEFT_SHIFT, code));
    let plain = |code| Some((0, code));
    match ch {
        'a'..='z' => plain(0x04 + (ch as u8 - b'a')),
        'A'..='Z' => shifted(0x04 + (ch as u8 - b'A')),
        '1'..='9' => plain(0x1e + (ch as u8 - b'1')),
        '0' => plain(0x27),
        '\n' | '\r' => plain(0x28),
        '\t' => plain(0x2b),
        ' ' => plain(0x2c),
        '-' => plain(0x2d),
        '=' => plain(0x2e),
        '[' => plain(0x2f),
        ']' => plain(0x30),
        '\\' => plain(0x31),
        ';' => plain(0x33),
        '\'' => plain(0x34),
        '`' => plain(0x35),
        ',' => plain(0x36),
        '.' => plain(0x37),
        '/' => plain(0x38),
        '!' => shifted(0x1e),
        '@' => shifted(0x1f),
        '#' => shifted(0x20),
        '$' => shifted(0x21),
        '%' => shifted(0x22),
        '^' => shifted(0x23),
        '&' => shifted(0x24),
        '*' => shifted(0x25),
        '(' => shifted(0x26),
        ')' => shifted(0x27),
        '_' => shifted(0x2d),
        '+' => shifted(0x2e),
        '{' => shifted(0x2f),
        '}' => shifted(0x30),
        '|' => shifted(0x31),
        ':' => shifted(0x33),
        '"' => shifted(0x34),
        '~' => shifted(0x35),
        '<' => shifted(0x36),
        '>' => shifted(0x37),
        '?' => shifted(0x38),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT_CTRL: u8 = 1 << 0;

    #[test]
    fn keys_resolve_like_the_reference() {
        assert_eq!(resolve("KeyA"), Ok((0x04, false)));
        assert_eq!(resolve("ctrl"), Ok((LEFT_CTRL, true)));
        assert_eq!(resolve("Win"), Ok((1 << 3, true)));
        assert_eq!(resolve("enter"), Ok((0x28, false)));
        assert_eq!(resolve("F12"), Ok((0x45, false)));
        assert_eq!(resolve("F13"), Ok((0x68, false)));
        assert_eq!(resolve("0"), Ok((0x27, false)));
        assert_eq!(resolve("Numpad0"), Ok((0x62, false)));
        assert!(resolve("Hyper").is_err());
        assert_eq!(
            hotkey_report(&["ctrl".into(), "alt".into(), "Delete".into()]),
            Ok([LEFT_CTRL | (1 << 2), 0, 0x4c, 0, 0, 0, 0, 0])
        );
        assert!(hotkey_report(&[]).is_err());
        let seven: Vec<String> = "abcdefg".chars().map(String::from).collect();
        assert!(hotkey_report(&seven).is_err());
        assert_eq!(character('A'), Some((LEFT_SHIFT, 0x04)));
        assert_eq!(character('?'), Some((LEFT_SHIFT, 0x38)));
        assert_eq!(character('ж'), None);
    }
}
