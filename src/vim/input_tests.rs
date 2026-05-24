use gpui::{Keystroke, Modifiers};

use super::{input::vim_key_from_keystroke, VimKey};

fn keystroke(key: &str) -> Keystroke {
    Keystroke {
        modifiers: Modifiers::default(),
        key: key.to_string(),
        key_char: Some(key.to_string()),
    }
}

#[test]
fn maps_plain_vim_keystrokes() {
    assert_eq!(
        vim_key_from_keystroke(&keystroke("j")),
        Some(VimKey::Char('j'))
    );
    assert_eq!(
        vim_key_from_keystroke(&keystroke("G")),
        Some(VimKey::Char('G'))
    );
    assert_eq!(
        vim_key_from_keystroke(&keystroke("$")),
        Some(VimKey::Char('$'))
    );
}

#[test]
fn maps_navigation_and_control_keys() {
    assert_eq!(
        vim_key_from_keystroke(&keystroke("down")),
        Some(VimKey::Down)
    );
    assert_eq!(
        vim_key_from_keystroke(&Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            key: "d".to_string(),
            key_char: None,
        }),
        Some(VimKey::Ctrl('d'))
    );
}

#[test]
fn ignores_platform_modified_and_unknown_keys() {
    assert_eq!(
        vim_key_from_keystroke(&Keystroke {
            modifiers: Modifiers {
                platform: true,
                ..Default::default()
            },
            key: "j".to_string(),
            key_char: None,
        }),
        None
    );
    assert_eq!(vim_key_from_keystroke(&keystroke("a")), None);
}
