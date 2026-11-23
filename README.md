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

## Features

- **Fuzzy search** across history (`f` or `/`), with fzf query syntax
  (`'exact`, `^prefix`, `suffix$`, `!exclude`) and smart case. Matches are
  highlighted. Search covers the preview text `cliphist list` shows, which is
  the first 100 characters of each entry.
- **Preview pane**: the full text of the highlighted entry (which
  `cliphist list` cuts off), and image thumbnails via the kitty graphics
  protocol. Thumbnails are cached on disk.
- **Pins** (`p`): keep entries at the top. They survive re-copies, cliphist
  trimming old entries, and `cliphist wipe`.
- **Delete** (`dd`): press `d` twice so nothing is deleted by accident.
  Mark several entries with `Space` first to delete them together.
- **Wipe** (`WW`): clear cliphist's history from clipdeck. Pins are kept.
- **Enter** copies the entry and exits.
- **Live reload** when history changes while the popup is open.
- Vim-style keys plus arrow keys. Press `?` for the full list.

## Planned

- Type filters: text, image, URL
- Paste the entry straight into the previous window via `wtype`
- Configurable keybinds and colors
- `--pick` mode: print the selected entry to stdout, for use in scripts

## Requirements

- A Wayland compositor (developed on Hyprland)
- `cliphist` and `wl-clipboard` (`wl-copy`, `wl-paste`)
- A terminal with the kitty graphics protocol for image previews (Ghostty,
  kitty, WezTerm). Other terminals fall back to a
  lower-resolution preview drawn with block characters.
- Rust, to build it (install through [rustup](https://rustup.rs) or your
  distro's package)

## Install

```bash
git clone https://github.com/TheyCallMePBJ/clipdeck
cd clipdeck
cargo install --path .
```

This builds clipdeck and puts the binary in `~/.cargo/bin/clipdeck`. To
update later, `git pull` and run `cargo install --path .` again.

> **PATH note:** `~/.cargo/bin` is often added to `PATH` only by your shell's
> config, which your compositor doesn't read. If a keybind reports that
> `clipdeck` can't be found, use the full path in the bind
> (`ghostty --title=clipdeck -e /home/you/.cargo/bin/clipdeck`) or add
> `~/.cargo/bin` to your session's `PATH`.

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

clipdeck stores no history of its own, only pins.

- **Pins are keyed by a blake3 content hash, not cliphist ID.** cliphist IDs
  aren't stable: copying something that's already in history moves it to the
  top with a new ID.
- **Pinned content is stored alongside the hash**, so pins survive cliphist
  trimming old entries and `cliphist wipe`. Unpinning something cliphist no
  longer has puts it back into history, so it isn't lost.
- Thumbnails are cached in `~/.cache/clipdeck/` by content hash, so large
  screenshots aren't decoded again every time it opens.

### Paths (XDG)

- Config (planned): `~/.config/clipdeck/config.toml`
- Pins: `~/.local/share/clipdeck/pins.toml` (the list) and
  `~/.local/share/clipdeck/pins/<hash>` (each pin's content)
- Thumbnail cache: `~/.cache/clipdeck/thumbs/`

## Stack

| Concern         | Crate                       |
|-----------------|-----------------------------|
| TUI             | `ratatui` + `crossterm`     |
| Fuzzy matching  | `nucleo-matcher`            |
| Image previews  | `ratatui-image` + `image`   |
| Watching the DB | `notify`                    |
| Content hashing | `blake3`                    |
| Pins file       | `serde` + `toml`            |
| XDG paths       | `directories`               |
| CLI             | `clap`                      |
| Running cliphist / wl-copy | `std::process::Command` |

## Roadmap

- [x] **Stub and launch check**: `cliphist list` in a ratatui list, the
  Hyprland bind and window rules, and Ghostty startup time (about 10 ms to
  the first frame).
- [x] **Core loop**: navigation, copy-and-exit, delete, re-listing after
  every change.
- [x] **Search**: fuzzy filtering with `nucleo-matcher`.
- [x] **Preview pane**: full text, image thumbnails, and the thumbnail cache.
- [x] **Live reload**: watching cliphist's database with `notify`.
- [x] **Pins**: hash-keyed pin store and a pinned section at the top.
- [ ] **Polish** (in progress): multi-select delete, wipe with confirmation
  and a help screen are done; type filters, `--pick`, `wtype` paste, and
  configurable keybinds and colors are next.
- [ ] **Release**: README GIF (`vhs`), AUR package, crates.io.

## Non-goals

- Replacing cliphist's storage or running a clipboard daemon
- A GTK/Qt GUI (maybe later as a separate frontend)
- X11 support

## License

MIT, see [LICENSE](LICENSE).
