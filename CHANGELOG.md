# Changelog

What changed in each version of tradu.

## Unreleased

### Added

- tradu's hotkeys now work in games that block Windows hotkeys, like League of Legends.
- Drag empty space to move the popup, and drag its edges to resize it. Both are remembered like `:move` and `:resize`.
- `:help` scrolls with j/k and jumps with g/G.
- `install-local.ps1` builds and installs your checkout, for trying changes before a release.

### Changed

- tradu is now licensed under the GNU GPL v3 or later (GPL-3.0-or-later), copyright Kayf; it was MIT.
- `:` always opens the command line, and whatever you'd typed comes back afterwards.

### Fixed

- The first Space you typed after closing tradu with its hotkey could get swallowed. The hotkey's Space counted as held until tradu saw it come back up, which it doesn't always see once the popup has closed. Now it only counts as held while you're still holding the hotkey.

### Development

- Formatting (rustfmt, Prettier), linting (clippy, ESLint), type checking (TypeScript through JSDoc) and tests, all run by GitHub Actions on every push to main and every pull request.
