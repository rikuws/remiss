use gpui::Modifiers;

pub fn secondary_modifier_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    }
}

pub fn secondary_key_label(key: &str) -> String {
    format!("{}-{key}", secondary_modifier_label())
}

pub fn secondary_shift_key_label(key: &str) -> String {
    format!("{}-shift-{key}", secondary_modifier_label())
}

pub fn secondary_plain_modifier(modifiers: Modifiers) -> bool {
    secondary_modifier_only(modifiers, false)
}

pub fn secondary_shift_modifier(modifiers: Modifiers) -> bool {
    secondary_modifier_only(modifiers, true)
}

pub fn secondary_text_modifier(modifiers: Modifiers) -> bool {
    modifiers.secondary()
        && !modifiers.alt
        && !modifiers.function
        && secondary_has_no_conflicting_primary_modifier(modifiers)
}

fn secondary_modifier_only(modifiers: Modifiers, shift: bool) -> bool {
    secondary_text_modifier(modifiers) && modifiers.shift == shift
}

fn secondary_has_no_conflicting_primary_modifier(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        !modifiers.control
    } else {
        !modifiers.platform
    }
}
