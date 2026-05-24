//! Small read-only Vim command parser for review surfaces.
//!
//! This is intentionally not a text editor. It parses normal/visual-line Vim
//! movement commands into host-owned motions so diff and source views can decide
//! how rows, anchors, and scroll positions map to those motions.

pub mod diff;
pub mod input;

#[cfg(test)]
mod diff_tests;
#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReadOnlyVimMode {
    #[default]
    Normal,
    VisualLine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimKey {
    Char(char),
    Escape,
    Enter,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Ctrl(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimMotion {
    CharLeft,
    CharRight,
    LineUp,
    LineDown,
    LineStart,
    FirstNonBlank,
    LineEnd,
    WordForward,
    WordBackward,
    WordEnd,
    HalfPageUp,
    HalfPageDown,
    PageUp,
    PageDown,
    DocumentStart,
    DocumentEnd,
    AbsoluteLine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimIntent {
    Move { motion: VimMotion, count: usize },
    StartVisualLine,
    Cancel,
    ConfirmSelection,
    Noop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingKey {
    G,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOnlyVim {
    mode: ReadOnlyVimMode,
    count: Option<usize>,
    pending: Option<PendingKey>,
}

impl ReadOnlyVim {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> ReadOnlyVimMode {
        self.mode
    }

    pub fn has_pending_input(&self) -> bool {
        self.count.is_some() || self.pending.is_some()
    }

    pub fn reset(&mut self) {
        self.mode = ReadOnlyVimMode::Normal;
        self.count = None;
        self.pending = None;
    }

    pub fn enter_visual_line(&mut self) -> VimIntent {
        self.mode = ReadOnlyVimMode::VisualLine;
        self.count = None;
        self.pending = None;
        VimIntent::StartVisualLine
    }

    pub fn handle_key(&mut self, key: VimKey) -> VimIntent {
        match key {
            VimKey::Escape => {
                self.mode = ReadOnlyVimMode::Normal;
                self.count = None;
                self.pending = None;
                VimIntent::Cancel
            }
            VimKey::Char(character) if character.is_ascii_digit() => self.handle_digit(character),
            _ => {
                if self.pending.is_some() {
                    return self.handle_pending_key(key);
                }
                self.handle_command_key(key)
            }
        }
    }

    fn handle_digit(&mut self, character: char) -> VimIntent {
        if character == '0' && self.count.is_none() {
            return self.emit_motion(VimMotion::LineStart);
        }

        let digit = character.to_digit(10).unwrap_or(0) as usize;
        let next = self
            .count
            .unwrap_or(0)
            .saturating_mul(10)
            .saturating_add(digit)
            .max(1);
        self.count = Some(next);
        VimIntent::Noop
    }

    fn handle_pending_key(&mut self, key: VimKey) -> VimIntent {
        let pending = self.pending.take();
        match (pending, key) {
            (Some(PendingKey::G), VimKey::Char('g')) => {
                if self.count.is_some() {
                    self.emit_motion(VimMotion::AbsoluteLine)
                } else {
                    self.emit_motion(VimMotion::DocumentStart)
                }
            }
            _ => {
                self.count = None;
                VimIntent::Noop
            }
        }
    }

    fn handle_command_key(&mut self, key: VimKey) -> VimIntent {
        match key {
            VimKey::Char('h') | VimKey::Left => self.emit_motion(VimMotion::CharLeft),
            VimKey::Char('l') | VimKey::Right => self.emit_motion(VimMotion::CharRight),
            VimKey::Char('j') | VimKey::Down => self.emit_motion(VimMotion::LineDown),
            VimKey::Char('k') | VimKey::Up => self.emit_motion(VimMotion::LineUp),
            VimKey::Char('^') => self.emit_motion(VimMotion::FirstNonBlank),
            VimKey::Char('$') | VimKey::End => self.emit_motion(VimMotion::LineEnd),
            VimKey::Char('w') => self.emit_motion(VimMotion::WordForward),
            VimKey::Char('b') => self.emit_motion(VimMotion::WordBackward),
            VimKey::Char('e') => self.emit_motion(VimMotion::WordEnd),
            VimKey::Ctrl('u') => self.emit_motion(VimMotion::HalfPageUp),
            VimKey::Ctrl('d') => self.emit_motion(VimMotion::HalfPageDown),
            VimKey::Ctrl('b') | VimKey::PageUp => self.emit_motion(VimMotion::PageUp),
            VimKey::Ctrl('f') | VimKey::PageDown => self.emit_motion(VimMotion::PageDown),
            VimKey::Char('G') => {
                if self.count.is_some() {
                    self.emit_motion(VimMotion::AbsoluteLine)
                } else {
                    self.emit_motion(VimMotion::DocumentEnd)
                }
            }
            VimKey::Char('g') => {
                self.pending = Some(PendingKey::G);
                VimIntent::Noop
            }
            VimKey::Char('v') | VimKey::Char('V') => {
                if self.mode == ReadOnlyVimMode::VisualLine {
                    self.mode = ReadOnlyVimMode::Normal;
                    self.count = None;
                    VimIntent::Cancel
                } else {
                    self.enter_visual_line()
                }
            }
            VimKey::Enter | VimKey::Char('c') if self.mode == ReadOnlyVimMode::VisualLine => {
                self.mode = ReadOnlyVimMode::Normal;
                self.count = None;
                VimIntent::ConfirmSelection
            }
            VimKey::Home => self.emit_motion(VimMotion::LineStart),
            _ => {
                self.count = None;
                VimIntent::Noop
            }
        }
    }

    fn emit_motion(&mut self, motion: VimMotion) -> VimIntent {
        let count = self.count.take().unwrap_or(1).max(1);
        self.pending = None;
        VimIntent::Move { motion, count }
    }
}
