# clipdeck

A terminal-styled clipboard history manager for Wayland, built on top of
[cliphist](https://github.com/sentriz/cliphist). It opens as a floating popup
(a TUI running in a terminal window), lets you fuzzy-search your history with
text and image previews, and keeps everything in sync with cliphist.

## Why

Some launcher clipboard integrations keep their own copy of the history, so
deleting an entry in one place leaves it in the other. clipdeck never keeps
its own copy of the history: **cliphist is the single source of truth**.
Whatever you see in clipdeck is what cliphist holds, and every change goes
straight through cliphist.

## Features (v1)

- **Fuzzy search** across history (fzf-like matching via `nucleo-matcher`)
- **Preview pane**: full text for text entries, which `cliphist list` cuts
  off, and real image thumbnails via the kitty graphics protocol
- **Pins**: keep entries at the top; they survive re-copies, trimming and
  `cliphist wipe`
- **Delete / multi-select delete / wipe** (wipe asks for confirmation)
- **Type filters**: text, image, URL
- **Enter** copies the entry and exits; optionally pastes it directly via `wtype`
- **Live reload** when history changes while the popup is open
- Vim-style keybinds plus arrow keys, all configurable
- `--pick` mode: print the selected entry to stdout, for use in scripts

## Requirements

- A Wayland compositor (developed on Hyprland)
- `cliphist` and `wl-clipboard` (`wl-copy`, `wl-paste`)
- A terminal with the kitty graphics protocol for image previews (Ghostty,
  kitty, WezTerm). Other terminals fall back to text-only previews.
- Optional: `wtype`, for paste-on-select

## Setup

cliphist needs to be recording history already:

```bash
# Run at login, however your setup autostarts programs
wl-paste --type text --watch cliphist store
wl-paste --type image --watch cliphist store
```

Bind clipdeck to a key and float its window (Hyprland Lua config, 0.55+):

```lua
-- hyprland.lua
hl.bind("SUPER + V", hl.dsp.exec_cmd("ghostty --title=clipdeck -e clipdeck"))

hl.window_rule({
  name         = "clipdeck-popup",
  match        = { title = "^clipdeck$" },
  float        = true,
  center       = true,
  size         = { 900, 600 },
  stay_focused = true,
  dim_around   = true,  -- optional
})
```

Static rules such as `float`, `center` and `size` are matched against the
window's *initial* title, so the title has to be set at launch with
`--title` rather than by the app after it starts.

> **Ghostty note:** Ghostty starts much faster when it reuses a running
> instance (`gtk-single-instance`). A custom `--class` likely starts a fresh,
> slow instance, so match the window by **title** instead. Measure the
> startup time early.

## Design

### cliphist is the source of truth

| Action        | How                                                      |
|---------------|----------------------------------------------------------|
| List          | `cliphist list`, run fresh every time the app opens      |
| Preview       | `cliphist decode`, only for the highlighted entry (lazy) |
| Copy          | `cliphist decode \| wl-copy`                             |
| Delete        | send the original `cliphist list` line to `cliphist delete` |
| Wipe          | `cliphist wipe`                                          |
| Stay in sync  | watch cliphist's DB (`~/.cache/cliphist/db` by default) with `notify`, then re-list |

After any change, list again rather than editing the UI's copy, so the
display always matches what cliphist actually holds.

clipdeck calls the cliphist commands rather than reading its database directly,
because the database format is internal to cliphist and may change.

### Owned state: metadata only

clipdeck stores no history of its own, only metadata (currently just pins).

- **Pins are keyed by a blake3 content hash, not cliphist ID.** cliphist IDs
  aren't stable: copying something that's already in history moves it to the
  top with a new ID.
- **Pinned content is stored alongside the hash**, so pins survive cliphist
  trimming old entries and `cliphist wipe`.
- Thumbnails are cached in `~/.cache/clipdeck/` by content hash, so large
  screenshots aren't decoded again every time it opens.

### Paths (XDG)

- Config: `~/.config/clipdeck/config.toml`
- Pins: `~/.local/share/clipdeck/pins.toml`
- Thumbnail cache: `~/.cache/clipdeck/thumbs/`

## Stack

| Concern         | Crate                       |
|-----------------|-----------------------------|
| TUI             | `ratatui` + `crossterm`     |
| Fuzzy matching  | `nucleo-matcher`            |
| Image previews  | `ratatui-image` + `image`   |
| Watching the DB | `notify`                    |
| Content hashing | `blake3`                    |
| Config / pins   | `serde` + `toml`            |
| XDG paths       | `directories`               |
| CLI             | `clap`                      |
| Running cliphist / wl-copy | `std::process::Command` |

## Roadmap

1. **Stub and launch check**: a binary that prints `cliphist list` in a ratatui
   list. Wire up the Hyprland bind and rules, and measure Ghostty startup time.
2. **Core loop**: navigation, copy-and-exit, delete, re-list after changes.
3. **Search**: fuzzy filtering with `nucleo-matcher`.
4. **Preview pane**: full text first, then image thumbnails and the thumbnail cache.
5. **Live reload**: watch the DB with `notify`.
6. **Pins**: hash-keyed pin store and a pinned section at the top.
7. **Polish**: multi-select, type filters, wipe confirmation, `--pick`,
   `wtype` paste, configurable keybinds and colors.
8. **Release**: README GIF (`vhs`), AUR package, `cargo install`.

## Non-goals

- Replacing cliphist's storage or running a clipboard daemon
- A GTK/Qt GUI (maybe later as a separate frontend)
- X11 support

## License

MIT, see [LICENSE](LICENSE).
