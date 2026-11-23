//! Key bindings: which key runs which command.
//!
//! Keys are written the way the config file spells them: a key name,
//! optionally after modifiers joined with `+`. For example `j`, `G`, `?`,
//! `Enter`, `Space`, `ctrl+d`, `shift+Down` and `shift+Tab`.

use std::collections::HashMap;
use std::fmt;

use anyhow::{Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Everything a key can be bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    Down,
    Up,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    Top,
    Bottom,
    PreviewDown,
    PreviewUp,
    Select,
    Search,
    Pin,
    Mark,
    Delete,
    Wipe,
    NextType,
    PreviousType,
    Reload,
    Help,
    Back,
    Quit,
}

impl Command {
    /// Every command with its name in the config file, in help order.
    pub const ALL: [(Command, &'static str); 22] = [
        (Command::Down, "down"),
        (Command::Up, "up"),
        (Command::HalfPageDown, "half_page_down"),
        (Command::HalfPageUp, "half_page_up"),
        (Command::PageDown, "page_down"),
        (Command::PageUp, "page_up"),
        (Command::Top, "top"),
        (Command::Bottom, "bottom"),
        (Command::PreviewDown, "preview_down"),
        (Command::PreviewUp, "preview_up"),
        (Command::Select, "select"),
        (Command::Search, "search"),
        (Command::Pin, "pin"),
        (Command::Mark, "mark"),
        (Command::Delete, "delete"),
        (Command::Wipe, "wipe"),
        (Command::NextType, "next_type"),
        (Command::PreviousType, "previous_type"),
        (Command::Reload, "reload"),
        (Command::Help, "help"),
        (Command::Back, "back"),
        (Command::Quit, "quit"),
    ];

    pub fn from_name(name: &str) -> Option<Command> {
        Self::ALL
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(command, _)| *command)
    }

    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(c, _)| *c == self)
            .map(|(_, name)| *name)
            .expect("every command is in ALL")
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|(c, _)| *c == self)
            .expect("every command is in ALL")
    }
}

/// One key press: a key plus Ctrl, Alt and Shift.
///
/// Shift is folded into the key where the terminal already does that: `J`
/// rather than `shift+j`, and `shift+Tab` is the terminal's BackTab key.
/// That way the same press always gives the same `Key`, however it was
/// written in the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl Key {
    fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        let mut modifiers =
            modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        let code = match code {
            KeyCode::Char(c) if modifiers.contains(KeyModifiers::SHIFT) => {
                modifiers.remove(KeyModifiers::SHIFT);
                KeyCode::Char(c.to_ascii_uppercase())
            }
            KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => {
                modifiers.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            KeyCode::BackTab => {
                modifiers.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            other => other,
        };
        Self { code, modifiers }
    }

    pub fn from_event(event: KeyEvent) -> Self {
        // Terminals send shifted letters as the uppercase letter, sometimes
        // with Shift set and sometimes not. `new` treats both the same.
        Self::new(event.code, event.modifiers)
    }

    /// Parses a key as written in the config file.
    pub fn parse(text: &str) -> Result<Self> {
        // `+` separates modifiers, so the `+` key itself is written `+`
        // (or `ctrl++`).
        let (modifier_text, key_text) = if text == "+" {
            ("", "+")
        } else if let Some(mods) = text.strip_suffix("++") {
            (mods, "+")
        } else {
            match text.rsplit_once('+') {
                Some((mods, key)) => (mods, key),
                None => ("", text),
            }
        };

        let mut modifiers = KeyModifiers::NONE;
        if !modifier_text.is_empty() {
            for part in modifier_text.split('+') {
                modifiers |= match part.to_ascii_lowercase().as_str() {
                    "ctrl" | "control" => KeyModifiers::CONTROL,
                    "alt" => KeyModifiers::ALT,
                    "shift" => KeyModifiers::SHIFT,
                    _ => bail!("unknown modifier {part:?} in key {text:?}"),
                };
            }
        }

        let mut chars = key_text.chars();
        let code = match (chars.next(), chars.next()) {
            (None, _) => bail!("key {text:?} has no key after the modifiers"),
            (Some(c), None) => KeyCode::Char(c),
            _ => named_key(key_text).ok_or_else(|| {
                anyhow::anyhow!("unknown key {key_text:?} in {text:?}")
            })?,
        };
        Ok(Self::new(code, modifiers))
    }

    /// The character, for a letter, digit, symbol or space typed without
    /// Ctrl or Alt. While searching, these go into the query instead of
    /// running commands.
    pub fn plain_char(self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) if self.modifiers.is_empty() => Some(c),
            _ => None,
        }
    }
}

fn named_key(name: &str) -> Option<KeyCode> {
    Some(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "plus" => KeyCode::Char('+'),
        other => {
            let n: u8 = other.strip_prefix('f')?.parse().ok()?;
            if !(1..=24).contains(&n) {
                return None;
            }
            KeyCode::F(n)
        }
    })
}

/// Writes the key back the way the config file spells it.
impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            f.write_str("ctrl+")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            f.write_str("alt+")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            f.write_str("shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("Space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => f.write_str("Enter"),
            KeyCode::Esc => f.write_str("Esc"),
            KeyCode::Tab => f.write_str("Tab"),
            KeyCode::BackTab => f.write_str("shift+Tab"),
            KeyCode::Backspace => f.write_str("Backspace"),
            KeyCode::Delete => f.write_str("Delete"),
            KeyCode::Insert => f.write_str("Insert"),
            KeyCode::Home => f.write_str("Home"),
            KeyCode::End => f.write_str("End"),
            KeyCode::PageUp => f.write_str("PageUp"),
            KeyCode::PageDown => f.write_str("PageDown"),
            KeyCode::Up => f.write_str("Up"),
            KeyCode::Down => f.write_str("Down"),
            KeyCode::Left => f.write_str("Left"),
            KeyCode::Right => f.write_str("Right"),
            KeyCode::F(n) => write!(f, "F{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// The bindings in effect.
#[derive(Debug, Clone)]
pub struct Keymap {
    commands: HashMap<Key, Command>,
    /// The keys of each command, in `Command::ALL` order. The first is the
    /// one shown in hints such as "press d again".
    keys: Vec<Vec<Key>>,
}

impl Keymap {
    /// Builds a keymap from `(command name, keys)` pairs: first the
    /// defaults, then the user's.
    ///
    /// A command the user lists gets exactly the keys they give (an empty
    /// list unbinds it). A key the user binds is taken away from whichever
    /// default command had it, so rebinding `d` doesn't mean also having to
    /// unbind it from `delete`. Two of the user's own commands sharing a key
    /// is an error, since only one of them could run.
    pub fn build(defaults: &[(String, Vec<String>)], user: &[(String, Vec<String>)]) -> Result<Self> {
        let mut keys: Vec<Vec<Key>> = vec![Vec::new(); Command::ALL.len()];
        for (name, list) in defaults {
            keys[command(name)?.index()] = parse_all(list)?;
        }

        let mut user_commands = Vec::new();
        for (name, list) in user {
            let command = command(name)?;
            keys[command.index()] = parse_all(list)?;
            user_commands.push(command);
        }
        let user_keys: Vec<Key> = user_commands
            .iter()
            .flat_map(|c| keys[c.index()].clone())
            .collect();
        for (command, _) in Command::ALL {
            if !user_commands.contains(&command) {
                keys[command.index()].retain(|k| !user_keys.contains(k));
            }
        }

        let mut commands = HashMap::new();
        for (command, _) in Command::ALL {
            for key in &keys[command.index()] {
                if let Some(other) = commands.insert(*key, command)
                    && other != command
                {
                    bail!(
                        "key {key} is bound to both {} and {}",
                        other.name(),
                        command.name()
                    );
                }
            }
        }
        Ok(Self { commands, keys })
    }

    pub fn command(&self, key: Key) -> Option<Command> {
        self.commands.get(&key).copied()
    }

    pub fn keys(&self, command: Command) -> &[Key] {
        &self.keys[command.index()]
    }

    /// The first key of `command`, as text, if it has any.
    pub fn first(&self, command: Command) -> Option<String> {
        self.keys(command).first().map(Key::to_string)
    }

    /// How to write pressing `command`'s key twice: `dd`, or
    /// `Delete Delete` for a named key.
    pub fn twice(&self, command: Command) -> Option<String> {
        let key = self.first(command)?;
        Some(if key.chars().count() == 1 {
            format!("{key}{key}")
        } else {
            format!("{key} {key}")
        })
    }
}

fn command(name: &str) -> Result<Command> {
    Command::from_name(name).ok_or_else(|| {
        let names: Vec<&str> = Command::ALL.iter().map(|(_, n)| *n).collect();
        anyhow::anyhow!(
            "unknown command {name:?} in [keys]; the commands are: {}",
            names.join(", ")
        )
    })
}

fn parse_all(list: &[String]) -> Result<Vec<Key>> {
    list.iter().map(|k| Key::parse(k)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Key {
        Key::parse(text).unwrap()
    }

    fn event(code: KeyCode, modifiers: KeyModifiers) -> Key {
        Key::from_event(KeyEvent::new(code, modifiers))
    }

    fn pairs(list: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        list.iter()
            .map(|(name, keys)| {
                (
                    name.to_string(),
                    keys.iter().map(|k| k.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn parses_what_the_terminal_sends() {
        assert_eq!(key("j"), event(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(key("G"), event(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert_eq!(key("G"), event(KeyCode::Char('G'), KeyModifiers::NONE));
        assert_eq!(key("shift+g"), key("G"));
        assert_eq!(key("ctrl+d"), event(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(key("Space"), event(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(key("shift+Tab"), event(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(key("shift+Down"), event(KeyCode::Down, KeyModifiers::SHIFT));
        assert_eq!(key("PageDown"), key("pgdn"));
        assert_eq!(key("F5"), event(KeyCode::F(5), KeyModifiers::NONE));
        assert_eq!(key("+"), event(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(key("ctrl++"), event(KeyCode::Char('+'), KeyModifiers::CONTROL));
    }

    #[test]
    fn displays_like_the_config_spells_it() {
        for text in ["j", "G", "?", "ctrl+d", "Space", "Enter", "shift+Tab", "shift+Down", "F5"] {
            assert_eq!(key(text).to_string(), text);
        }
    }

    #[test]
    fn bad_keys_are_errors() {
        for text in ["", "ctrl+", "hyper+j", "Enterr", "F30"] {
            assert!(Key::parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn user_bindings_replace_defaults_and_take_their_keys() {
        let defaults = pairs(&[("delete", &["d", "Delete"]), ("pin", &["p"])]);
        let user = pairs(&[("pin", &["d"])]);
        let map = Keymap::build(&defaults, &user).unwrap();
        assert_eq!(map.command(key("d")), Some(Command::Pin));
        assert_eq!(map.command(key("p")), None, "pin's default is replaced");
        assert_eq!(map.keys(Command::Delete), [key("Delete")]);
    }

    #[test]
    fn user_conflicts_and_unknown_names_are_errors() {
        let user = pairs(&[("pin", &["x"]), ("mark", &["x"])]);
        let err = Keymap::build(&[], &user).unwrap_err().to_string();
        assert!(err.contains("x is bound to both pin and mark"), "{err}");

        let user = pairs(&[("pinn", &["x"])]);
        let err = Keymap::build(&[], &user).unwrap_err().to_string();
        assert!(err.contains("unknown command \"pinn\""), "{err}");
    }

    #[test]
    fn empty_list_unbinds() {
        let defaults = pairs(&[("wipe", &["W"])]);
        let map = Keymap::build(&defaults, &pairs(&[("wipe", &[])])).unwrap();
        assert_eq!(map.command(key("W")), None);
        assert_eq!(map.twice(Command::Wipe), None);
    }

    #[test]
    fn twice() {
        let defaults = pairs(&[("delete", &["d"]), ("wipe", &["Delete"])]);
        let map = Keymap::build(&defaults, &[]).unwrap();
        assert_eq!(map.twice(Command::Delete).unwrap(), "dd");
        assert_eq!(map.twice(Command::Wipe).unwrap(), "Delete Delete");
    }
}
