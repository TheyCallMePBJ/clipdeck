# Demo recording

`clipdeck.gif` in the main README is recorded from `demo.tape` with
[vhs](https://github.com/charmbracelet/vhs). From the repository root:

```bash
cargo build --release
vhs demo/demo.tape
```

`setup.sh` builds a throwaway clipboard history (its own cliphist database
and XDG directories), so the recording never shows your real history, pins
or config. It needs `cliphist` and ImageMagick (`magick`).
