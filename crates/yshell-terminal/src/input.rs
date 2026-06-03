//! Terminal input events. Broadcast copies these events, not command strings.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalInputEvent {
    PrintableText {
        text: String,
    },
    ControlKey {
        key: ControlKey,
    },
    NavigationKey {
        key: NavigationKey,
    },
    FunctionKey {
        number: u8,
        modifiers: KeyModifiers,
    },
    ModifiedKey {
        key: String,
        modifiers: KeyModifiers,
    },
    Paste {
        bytes: Vec<u8>,
        bracketed: bool,
    },
    Resize {
        columns: u16,
        rows: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKey {
    Enter,
    Backspace,
    Tab,
    Escape,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationKey {
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}
