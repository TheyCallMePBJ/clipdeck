//! The config file, `~/.config/clipdeck/config.toml`.
//!
//! Every setting is optional. The defaults live in [`DEFAULT_CONFIG`],
//! which is also what `clipdeck --default-config` prints, so the documented
//! defaults and the real ones can't drift apart.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use ratatui::style::{Color, Modifier, Style};
use ratatui_image::picker::ProtocolType;
use serde::Deserialize;

use crate::keys::Keymap;

pub const DEFAULT_CONFIG: &str = r##"# clipdeck config: ~/.config/clipdeck/config.toml
#
# Everything is optional: leave out what you don't want to change.

[keys]
# Each command takes a key or a list of keys. Listing a command replaces all
# of its default keys, and an empty list ([]) unbinds it. A key you bind is
# taken away from the command that had it by default.
#
# Keys: a character (j, G, ?, /), Space, Enter, Esc, Tab, Backspace, Delete,
# Insert, Home, End, PageUp, PageDown, Up, Down, Left, Right, F1-F24, after
# any of ctrl+, alt+ and shift+ (e.g. ctrl+d, shift+Down, shift+Tab).
#
# While searching, characters typed without ctrl or alt go into the search,
# and Esc, Backspace, ctrl+w and ctrl+u edit it. The other bindings (Enter,
# arrows, Tab, ctrl+ keys) work there too. ctrl+c always quits.

down = ["j", "Down", "ctrl+n"]
up = ["k", "Up", "ctrl+p"]
half_page_down = "ctrl+d"
half_page_up = "ctrl+u"
page_down = "PageDown"
page_up = "PageUp"
top = ["g", "Home"]
bottom = ["G", "End"]
preview_down = ["J", "shift+Down"]
preview_up = ["K", "shift+Up"]
select = "Enter"            # copy, paste or print, then close
search = ["f", "/"]
pin = "p"
mark = "Space"
delete = ["d", "Delete"]    # press twice
wipe = "W"                  # press twice
next_type = "Tab"           # all, text, images, URLs
previous_type = "shift+Tab"
reload = "r"
help = "?"
back = "Esc"                # clear marks, then search, then type; then quit
quit = "q"

[colors]
# A name (red, light-blue, dark-gray, ...), "#rrggbb", a 0-255 palette
# index, or "reset" for the terminal's default color.

accent = "yellow"           # keys in the help screen
pin = "yellow"              # the star on pinned entries
mark = "magenta"            # the dot on marked entries
match = "yellow"            # matched characters while searching
url = "cyan"
border = "reset"
info = "green"              # status messages
prompt = "yellow"           # "press d again" and similar
error = "red"
# The selected row is drawn in reverse video unless you set these:
# selected_fg = "black"
# selected_bg = "yellow"

[preview]
# How images are drawn: "auto" (ask the terminal), "kitty", "sixel",
# "iterm2", or "halfblocks" (colored block characters, works anywhere).
# Set this if images don't show up although your terminal supports them.
image_protocol = "auto"
"##;

#[derive(Debug, Clone)]
pub struct Config {
    pub keymap: Keymap,
    pub theme: Theme,
    /// `None` means detect it by asking the terminal.
    pub image_protocol: Option<ProtocolType>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub accent: Color,
    pub pin: Color,
    pub mark: Color,
    pub matched: Color,
    pub url: Color,
    pub border: Color,
    pub info: Color,
    pub prompt: Color,
    pub error: Color,
    pub selected_fg: Option<Color>,
    pub selected_bg: Option<Color>,
}

impl Theme {
    pub fn selected(&self) -> Style {
        match (self.selected_fg, self.selected_bg) {
            (None, None) => Style::new().add_modifier(Modifier::REVERSED),
            (fg, bg) => {
                let mut style = Style::new();
                if let Some(fg) = fg {
                    style = style.fg(fg);
                }
                if let Some(bg) = bg {
                    style = style.bg(bg);
                }
                style
            }
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Config::default().theme
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::parse(DEFAULT_CONFIG, "").expect("the default config is valid")
    }
}

/// The file's layout. Unknown sections or color names are errors, so a
/// typo doesn't silently do nothing.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    keys: BTreeMap<String, OneOrMany>,
    #[serde(default)]
    colors: Colors,
    #[serde(default)]
    preview: Preview,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    image_protocol: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(key) => vec![key],
            OneOrMany::Many(keys) => keys,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Colors {
    accent: Option<String>,
    pin: Option<String>,
    mark: Option<String>,
    #[serde(rename = "match")]
    matched: Option<String>,
    url: Option<String>,
    border: Option<String>,
    info: Option<String>,
    prompt: Option<String>,
    error: Option<String>,
    selected_fg: Option<String>,
    selected_bg: Option<String>,
}

impl Config {
    /// Reads the config at `path`. A missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                Self::parse(DEFAULT_CONFIG, &text).with_context(|| format!("in {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("can't read {}", path.display())),
        }
    }

    /// Applies the `user` config on top of the `defaults` (both TOML).
    fn parse(defaults: &str, user: &str) -> Result<Self> {
        let defaults: File = toml::from_str(defaults).context("bad default config")?;
        let user: File = toml::from_str(user)?;
        let protocol = user
            .preview
            .image_protocol
            .as_deref()
            .or(defaults.preview.image_protocol.as_deref())
            .map(parse_protocol)
            .transpose()?
            .flatten();

        let split = |file: File| -> (Vec<(String, Vec<String>)>, Colors) {
            let keys = file
                .keys
                .into_iter()
                .map(|(name, keys)| (name, keys.into_vec()))
                .collect();
            (keys, file.colors)
        };
        let (default_keys, default_colors) = split(defaults);
        let (user_keys, user_colors) = split(user);
        let keymap = Keymap::build(&default_keys, &user_keys)?;

        // The user's color if given, otherwise the default one.
        let pick =
            |user: Option<String>, default: Option<String>, name: &str| match user.or(default) {
                Some(text) => parse_color(&text, name).map(Some),
                None => Ok(None),
            };
        let required = |user, default, name: &str| -> Result<Color> {
            Ok(pick(user, default, name)?.unwrap_or(Color::Reset))
        };
        let (u, d) = (user_colors, default_colors);
        let theme = Theme {
            accent: required(u.accent, d.accent, "accent")?,
            pin: required(u.pin, d.pin, "pin")?,
            mark: required(u.mark, d.mark, "mark")?,
            matched: required(u.matched, d.matched, "match")?,
            url: required(u.url, d.url, "url")?,
            border: required(u.border, d.border, "border")?,
            info: required(u.info, d.info, "info")?,
            prompt: required(u.prompt, d.prompt, "prompt")?,
            error: required(u.error, d.error, "error")?,
            selected_fg: pick(u.selected_fg, d.selected_fg, "selected_fg")?,
            selected_bg: pick(u.selected_bg, d.selected_bg, "selected_bg")?,
        };
        Ok(Self {
            keymap,
            theme,
            image_protocol: protocol,
        })
    }
}

fn parse_color(text: &str, name: &str) -> Result<Color> {
    Color::from_str(text).map_err(|_| anyhow::anyhow!("colors.{name}: unknown color {text:?}"))
}

/// `None` for "auto".
fn parse_protocol(text: &str) -> Result<Option<ProtocolType>> {
    Ok(match text.to_ascii_lowercase().as_str() {
        "auto" => None,
        "kitty" => Some(ProtocolType::Kitty),
        "sixel" => Some(ProtocolType::Sixel),
        "iterm2" => Some(ProtocolType::Iterm2),
        "halfblocks" => Some(ProtocolType::Halfblocks),
        _ => bail!(
            "preview.image_protocol: unknown protocol {text:?} \
             (use auto, kitty, sixel, iterm2 or halfblocks)"
        ),
    })
}

/// `~/.config/clipdeck/config.toml` (respecting `XDG_CONFIG_HOME`).
pub fn default_path() -> Option<PathBuf> {
    Some(
        directories::ProjectDirs::from("", "", "clipdeck")?
            .config_dir()
            .join("config.toml"),
    )
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::keys::{Command, Key};

    /// The config you'd get from a file containing `text`.
    pub fn user(text: &str) -> Result<Config> {
        Config::parse(DEFAULT_CONFIG, text)
    }

    #[test]
    fn defaults_parse_and_bind_every_command() {
        let config = Config::default();
        for (command, name) in Command::ALL {
            assert!(!config.keymap.keys(command).is_empty(), "{name} has no key");
        }
        assert_eq!(config.theme.pin, Color::Yellow);
        assert_eq!(
            config.theme.selected(),
            Style::new().add_modifier(Modifier::REVERSED)
        );
    }

    #[test]
    fn empty_file_is_the_defaults() {
        let config = user("").unwrap();
        assert_eq!(config.theme, Theme::default());
        assert_eq!(
            config.keymap.command(Key::parse("d").unwrap()),
            Some(Command::Delete)
        );
    }

    #[test]
    fn overrides_keys_and_colors() {
        let config = user(
            r##"
            [keys]
            delete = "x"
            pin = ["d", "ctrl+p"]
            [colors]
            pin = "#ff8800"
            selected_bg = "blue"
            "##,
        )
        .unwrap();
        let command = |k| config.keymap.command(Key::parse(k).unwrap());
        assert_eq!(command("x"), Some(Command::Delete));
        assert_eq!(command("d"), Some(Command::Pin));
        // ctrl+p was "up" by default; the user took it for pin.
        assert_eq!(command("ctrl+p"), Some(Command::Pin));
        assert_eq!(command("k"), Some(Command::Up));
        assert_eq!(config.theme.pin, Color::Rgb(0xff, 0x88, 0x00));
        assert_eq!(config.theme.mark, Color::Magenta, "untouched default");
        assert_eq!(config.theme.selected(), Style::new().bg(Color::Blue));
    }

    #[test]
    fn mistakes_are_errors() {
        assert!(user("[colours]\npin = \"red\"").is_err());
        assert!(user("[colors]\npinn = \"red\"").is_err());
        let err = format!("{:#}", user("[colors]\npin = \"reddish\"").unwrap_err());
        assert!(
            err.contains("colors.pin: unknown color \"reddish\""),
            "{err}"
        );
        assert!(user("[keys]\npin = \"hyper+p\"").is_err());
        assert!(user("[keys]\npin = 3").is_err());
    }

    #[test]
    fn image_protocol() {
        assert_eq!(user("").unwrap().image_protocol, None);
        let config = user("[preview]\nimage_protocol = \"halfblocks\"").unwrap();
        assert_eq!(config.image_protocol, Some(ProtocolType::Halfblocks));
        assert!(user("[preview]\nimage_protocol = \"png\"").is_err());
    }

    #[test]
    fn missing_file_is_the_defaults() {
        let config = Config::load(Path::new("/nonexistent/clipdeck/config.toml")).unwrap();
        assert_eq!(config.theme, Theme::default());
    }
}
