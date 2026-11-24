# Changelog

## 0.1.0

The first release.

- Browse cliphist's history in a TUI, with cliphist as the only source of
  truth: every change goes through cliphist and the list is re-read after.
- Fuzzy search with fzf syntax and highlighted matches.
- Preview pane with the full text of an entry, and image thumbnails (kitty,
  sixel, iTerm2 or block characters), cached on disk.
- Type filters for text, images and URLs.
- Pins that stay at the top and survive cliphist trimming and wiping.
- Delete with a double press, several at once with marks, and wipe.
- Live reload when history changes while clipdeck is open.
- `--paste` to paste into the window underneath with `wtype`.
- `--pick` to print the chosen entry to stdout for scripts.
- Configurable keybinds, colors and image protocol in
  `~/.config/clipdeck/config.toml`; `--default-config` prints the defaults.
