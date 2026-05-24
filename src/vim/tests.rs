use super::*;

fn char_key(character: char) -> VimKey {
    VimKey::Char(character)
}

#[test]
fn parses_basic_line_motions() {
    let mut vim = ReadOnlyVim::new();

    assert_eq!(
        vim.handle_key(char_key('j')),
        VimIntent::Move {
            motion: VimMotion::LineDown,
            count: 1
        }
    );
    assert_eq!(
        vim.handle_key(VimKey::Up),
        VimIntent::Move {
            motion: VimMotion::LineUp,
            count: 1
        }
    );
    assert_eq!(
        vim.handle_key(char_key('0')),
        VimIntent::Move {
            motion: VimMotion::LineStart,
            count: 1
        }
    );
    assert_eq!(
        vim.handle_key(char_key('$')),
        VimIntent::Move {
            motion: VimMotion::LineEnd,
            count: 1
        }
    );
}

#[test]
fn applies_count_prefix_to_next_motion() {
    let mut vim = ReadOnlyVim::new();

    assert_eq!(vim.handle_key(char_key('1')), VimIntent::Noop);
    assert_eq!(vim.handle_key(char_key('2')), VimIntent::Noop);
    assert_eq!(
        vim.handle_key(char_key('j')),
        VimIntent::Move {
            motion: VimMotion::LineDown,
            count: 12
        }
    );
    assert_eq!(
        vim.handle_key(char_key('k')),
        VimIntent::Move {
            motion: VimMotion::LineUp,
            count: 1
        }
    );
}

#[test]
fn distinguishes_zero_from_count_digit() {
    let mut vim = ReadOnlyVim::new();

    assert_eq!(
        vim.handle_key(char_key('0')),
        VimIntent::Move {
            motion: VimMotion::LineStart,
            count: 1
        }
    );
    assert_eq!(vim.handle_key(char_key('2')), VimIntent::Noop);
    assert_eq!(vim.handle_key(char_key('0')), VimIntent::Noop);
    assert_eq!(
        vim.handle_key(char_key('j')),
        VimIntent::Move {
            motion: VimMotion::LineDown,
            count: 20
        }
    );
}

#[test]
fn parses_document_and_absolute_line_motions() {
    let mut vim = ReadOnlyVim::new();

    assert_eq!(vim.handle_key(char_key('g')), VimIntent::Noop);
    assert_eq!(
        vim.handle_key(char_key('g')),
        VimIntent::Move {
            motion: VimMotion::DocumentStart,
            count: 1
        }
    );
    assert_eq!(
        vim.handle_key(char_key('G')),
        VimIntent::Move {
            motion: VimMotion::DocumentEnd,
            count: 1
        }
    );
    assert_eq!(vim.handle_key(char_key('4')), VimIntent::Noop);
    assert_eq!(vim.handle_key(char_key('2')), VimIntent::Noop);
    assert_eq!(
        vim.handle_key(char_key('G')),
        VimIntent::Move {
            motion: VimMotion::AbsoluteLine,
            count: 42
        }
    );
    assert_eq!(vim.handle_key(char_key('7')), VimIntent::Noop);
    assert_eq!(vim.handle_key(char_key('g')), VimIntent::Noop);
    assert_eq!(
        vim.handle_key(char_key('g')),
        VimIntent::Move {
            motion: VimMotion::AbsoluteLine,
            count: 7
        }
    );
}

#[test]
fn parses_page_motions() {
    let mut vim = ReadOnlyVim::new();

    assert_eq!(
        vim.handle_key(VimKey::Ctrl('d')),
        VimIntent::Move {
            motion: VimMotion::HalfPageDown,
            count: 1
        }
    );
    assert_eq!(
        vim.handle_key(VimKey::Ctrl('u')),
        VimIntent::Move {
            motion: VimMotion::HalfPageUp,
            count: 1
        }
    );
    assert_eq!(
        vim.handle_key(VimKey::PageDown),
        VimIntent::Move {
            motion: VimMotion::PageDown,
            count: 1
        }
    );
    assert_eq!(
        vim.handle_key(VimKey::Ctrl('b')),
        VimIntent::Move {
            motion: VimMotion::PageUp,
            count: 1
        }
    );
}

#[test]
fn visual_line_mode_confirms_or_cancels_selection() {
    let mut vim = ReadOnlyVim::new();

    assert_eq!(vim.handle_key(char_key('v')), VimIntent::StartVisualLine);
    assert_eq!(vim.mode(), ReadOnlyVimMode::VisualLine);
    assert_eq!(vim.handle_key(char_key('3')), VimIntent::Noop);
    assert_eq!(
        vim.handle_key(char_key('j')),
        VimIntent::Move {
            motion: VimMotion::LineDown,
            count: 3
        }
    );
    assert_eq!(vim.handle_key(char_key('c')), VimIntent::ConfirmSelection);
    assert_eq!(vim.mode(), ReadOnlyVimMode::Normal);

    assert_eq!(vim.handle_key(char_key('V')), VimIntent::StartVisualLine);
    assert_eq!(vim.handle_key(VimKey::Escape), VimIntent::Cancel);
    assert_eq!(vim.mode(), ReadOnlyVimMode::Normal);
}

#[test]
fn edit_commands_are_ignored_in_normal_mode() {
    let mut vim = ReadOnlyVim::new();

    assert_eq!(vim.handle_key(char_key('2')), VimIntent::Noop);
    assert_eq!(vim.handle_key(char_key('d')), VimIntent::Noop);
    assert_eq!(
        vim.handle_key(char_key('j')),
        VimIntent::Move {
            motion: VimMotion::LineDown,
            count: 1
        }
    );
    assert_eq!(vim.handle_key(char_key('x')), VimIntent::Noop);
    assert_eq!(vim.handle_key(char_key('o')), VimIntent::Noop);
}

#[test]
fn escape_clears_pending_prefixes() {
    let mut vim = ReadOnlyVim::new();

    assert_eq!(vim.handle_key(char_key('9')), VimIntent::Noop);
    assert_eq!(vim.handle_key(char_key('g')), VimIntent::Noop);
    assert_eq!(vim.handle_key(VimKey::Escape), VimIntent::Cancel);
    assert_eq!(
        vim.handle_key(char_key('j')),
        VimIntent::Move {
            motion: VimMotion::LineDown,
            count: 1
        }
    );
}
