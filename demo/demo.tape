# Records the README demo: `vhs demo/demo.tape` from the repository root,
# after `cargo build --release`. Uses a throwaway history (see setup.sh).

Output demo/clipdeck.gif


Set Shell "bash"
Set FontSize 18
Set Width 1300
Set Height 720
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 80ms

Hide
Type "source demo/setup.sh && clear"
Enter
Sleep 1s
Show

Type "clipdeck"
Sleep 300ms
Enter
Sleep 2.5s

# Move through history; the preview follows (full text, then an image).
Type "j"
Sleep 600ms
Type "j"
Sleep 600ms
Type "j"
Sleep 600ms
Type "j"
Sleep 600ms
Type "j"
Sleep 800ms
Type "jj"
Sleep 2.5s
Type "jj"
Sleep 2.5s

# Fuzzy search.
Type "f"
Sleep 400ms
Type "git"
Sleep 1.5s
Down
Sleep 1s
Escape
Sleep 400ms
Escape
Sleep 800ms

# Type filters: text, images, URLs.
Tab
Sleep 1.2s
Tab
Sleep 1.5s
Tab
Sleep 1.2s
Tab
Sleep 800ms

# Pin a URL to the top.
Type "G"
Sleep 600ms
Type "k"
Sleep 800ms
Type "p"
Sleep 2s

# Mark two entries.
Type "j"
Sleep 300ms
Space
Sleep 500ms
Space
Sleep 1.5s
Escape
Sleep 800ms

# The help screen.
Type "?"
Sleep 3.5s
Type "q"
Sleep 600ms
Type "q"
Sleep 500ms
