use gpui::Keystroke;

use super::VimKey;

pub fn vim_key_from_keystroke(keystroke: &Keystroke) -> Option<VimKey> {
    let modifiers = keystroke.modifiers;
    if modifiers.platform || modifiers.alt || modifiers.function {
        return None;
    }

    let key = keystroke.key.as_str();
    if modifiers.control {
        if modifiers.shift {
            return None;
        }
        let mut chars = key.chars();
        let character = chars.next()?.to_ascii_lowercase();
        if chars.next().is_none() {
            return Some(VimKey::Ctrl(character));
        }
        return None;
    }

    match key {
        "escape" => return Some(VimKey::Escape),
        "enter" => return Some(VimKey::Enter),
        "up" => return Some(VimKey::Up),
        "down" => return Some(VimKey::Down),
        "left" => return Some(VimKey::Left),
        "right" => return Some(VimKey::Right),
        "pageup" => return Some(VimKey::PageUp),
        "pagedown" => return Some(VimKey::PageDown),
        "home" => return Some(VimKey::Home),
        "end" => return Some(VimKey::End),
        _ => {}
    }

    let text = keystroke.key_char.as_deref().unwrap_or(key);
    let mut chars = text.chars();
    let character = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    match character {
        '0'..='9'
        | 'h'
        | 'j'
        | 'k'
        | 'l'
        | 'w'
        | 'b'
        | 'e'
        | 'g'
        | 'G'
        | 'v'
        | 'V'
        | '^'
        | '$'
        | 'c' => Some(VimKey::Char(character)),
        _ => None,
    }
}
