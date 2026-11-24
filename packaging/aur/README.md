# AUR package

The `PKGBUILD` for the [AUR](https://aur.archlinux.org) (not published
there yet). It builds the tagged release from GitHub; `makepkg -si` in this
directory builds and installs it. For a new version: bump `pkgver`
(and reset `pkgrel=1`), then run `updpkgsums` and
`makepkg --printsrcinfo > .SRCINFO` before pushing to the AUR.
