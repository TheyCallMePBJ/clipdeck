# AUR package

The `PKGBUILD` published as [clipdeck](https://aur.archlinux.org/packages/clipdeck).
It builds the tagged release from GitHub. For a new version: bump `pkgver`
(and reset `pkgrel=1`), then run `updpkgsums` and
`makepkg --printsrcinfo > .SRCINFO` before pushing to the AUR.
