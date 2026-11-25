# Galaxia

A glassy Wayland desktop with scrollable tiling and floating windows.

> Early development. Not usable yet.

## Layout
| Folder | What it is |
|---|---|
| `crates/galaxia-comp` | Compositor (Smithay): scrollable tiling, floating, glass blur |
| `crates/galaxia-shell` | Panel, launcher, control center, notifications |
| `crates/galaxia-theme` | Shared design tokens: colors, radius, spacing, glass |
| `docs/` | Ideas and inspiration |
| `scripts/` | Dev scripts |

Planned later: `galaxia-greeter`, `galaxia-settings`, `galaxia-session`.

## Build
```sh
./scripts/check.sh
```

## License
GPL-3.0-or-later
