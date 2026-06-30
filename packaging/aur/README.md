# AUR packaging for `wtcc`

This directory holds the source-of-truth `PKGBUILD` (and generated `.SRCINFO`) for the
[AUR](https://aur.archlinux.org/) package, mirroring the structure of
[`hyprflow`](https://aur.archlinux.org/packages/hyprflow). The build is a standard Rust
release-tarball package: it downloads the GitHub release tarball for the matching `v$pkgver`
tag, builds with `cargo build --frozen --release`, runs the test suite in `check()`, and
installs the `wtcc` binary plus the MIT license.

Runtime deps: `git`, `tmux`. Optional: `github-cli` (PR/CI badges), `claude-code` (the agent).

## Publishing a new version to the AUR

> **On every release**, `pkgver` in `PKGBUILD` must be bumped to the new `vX.Y.Z` and the
> checksum + `.SRCINFO` regenerated (steps 2–3 below). This is part of the release checklist —
> keep the in-repo `PKGBUILD`/`.SRCINFO` in sync with the latest tag so it never drifts.

The AUR package lives in a separate git repo on `aur.archlinux.org` under your account; it is
**not** pushed automatically from this repo. To publish (run on your Arch machine with your AUR
SSH key configured — the same setup you used for `hyprflow`):

```sh
# 1. Make sure the GitHub release tag vX.Y.Z exists (created from this repo).

# 2. Update pkgver in PKGBUILD to X.Y.Z, then refresh the checksum:
cd packaging/aur
updpkgsums                      # rewrites sha256sums from the release tarball
makepkg --printsrcinfo > .SRCINFO

# 3. Build & test locally before publishing:
makepkg -f                      # builds, runs check(), produces the package
namcap PKGBUILD *.pkg.tar.zst   # optional lint

# 4. Push to the AUR:
git clone ssh://aur@aur.archlinux.org/wtcc.git aur-wtcc   # first time only
cp PKGBUILD .SRCINFO aur-wtcc/
cd aur-wtcc
git add PKGBUILD .SRCINFO
git commit -m "wtcc X.Y.Z-1"
git push
```

After the first push the package appears at `https://aur.archlinux.org/packages/wtcc` and is
installable with any AUR helper (e.g. `paru -S wtcc` / `yay -S wtcc`).

> The `sha256sums` line in the committed `PKGBUILD` is filled in by `updpkgsums` against the
> real release tarball — never publish with the `REPLACE_WITH_…` placeholder.
