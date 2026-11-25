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

## Try it
Galaxia runs as a window inside your current desktop:
```sh
./scripts/run-nested.sh            # Alt+Enter opens a terminal
./scripts/run-nested.sh -c nautilus
```

| Keys | Action |
|---|---|
| Alt+Enter | Open terminal (`GALAXIA_TERM` to choose) |
| Alt+Left / Alt+Right (or H / L) | Move between columns |
| Alt+R | Cycle column width (1/3, 1/2, 2/3, full) |
| Alt+V | Float / unfloat window |
| Alt+[ | Stack window into the left column |
| Alt+Q | Close window |
| Alt+Shift+E | Quit Galaxia |

## License
GPL-3.0-or-later
