# Sourced by demo.tape. Builds a throwaway clipboard history so the
# recording never shows real history, pins or config:
#
# - a separate cliphist database (passed with --db-path)
# - XDG_* pointed at a temporary directory, so clipdeck's pins, thumbnail
#   cache and config (and cliphist's config) come from there too
#
# `clipdeck` is then a function that runs the release build against it.

DEMO=$(mktemp -d)
export XDG_DATA_HOME=$DEMO/data XDG_CACHE_HOME=$DEMO/cache XDG_CONFIG_HOME=$DEMO/config
BIN=$PWD/target/release/clipdeck

# vhs records a browser terminal that can't capture kitty or sixel images,
# so draw them with block characters.
mkdir -p "$XDG_CONFIG_HOME/clipdeck"
printf '[preview]\nimage_protocol = "halfblocks"\n' > "$XDG_CONFIG_HOME/clipdeck/config.toml"

store() { printf '%s' "$1" | cliphist -db-path "$DEMO/db" store; }

# Oldest first: the last one stored is at the top of the list.
store 'cargo install --path .'
store 'https://github.com/sentriz/cliphist'
store 'fn main() {
    let greeting = "hello, clipboard";
    for word in greeting.split(", ") {
        println!("{word}");
    }
}'
store 'git commit -m "Add type filters"'
magick -size 960x540 gradient:'#1e1e2e-#7aa2f7' \
    -gravity center -pointsize 190 -fill white -annotate 0 'clip' \
    png:- | cliphist -db-path "$DEMO/db" store
store 'https://ratatui.rs'
store 'Meeting moved to 3pm, room 4B'
store 'git push origin main'
store 'docker compose up -d'
store 'https://www.rust-lang.org/learn'
store 'ssh-keygen -t ed25519 -C "laptop"'
store 'git log --oneline --graph'

clipdeck() { "$BIN" --db-path "$DEMO/db" "$@"; }
