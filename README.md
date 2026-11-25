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
| Alt+Left / Right (or H / L) | Move between columns (the strip scrolls) |
| Alt+Up / Down (or K / J) | Move between windows stacked in a column |
| Alt+Shift+Left / Right | Move the column left / right |
| Alt+R | Cycle column width (1/3, 1/2, 2/3, full) |
| Alt+F | Toggle full width |
| Alt+[ / Alt+] | Stack window into the left column / pull it back out |
| Alt+V | Float / unfloat window |
| Alt+P | Pin a floating window to every workspace |
| Alt+drag | Move a floating window (a tiled one pops out to float) |
| Alt+scroll | Scroll the strip; Alt+Shift+scroll changes workspace |
| Alt+1 to 9 | Go to workspace |
| Alt+Shift+1 to 9 | Send window to workspace |
| Alt+U / I (or PageUp / PageDown) | Workspace up / down |
| Alt+Q | Close window |
| Alt+Shift+S | Screenshot of Galaxia to `~/Pictures/Galaxia/` |
| Alt+Shift+R | Reload theme |
| Alt+Shift+E | Quit Galaxia |

Dialogs float on their own. Apps with a see-through background (e.g. ghostty with
`background-opacity`) sit on frosted glass made from the blurred wallpaper.

 Apps are asked to drop their title bars so Galaxia draws the frame.

## Theme
Copy [docs/theme.example.toml](docs/theme.example.toml) to `~/.config/galaxia/theme.toml`.
Every setting is optional: colors, corner radius, border, gaps, shadow, animation speed, frosted
glass (blur, tint, behind windows and/or panels), and a
`wallpaper` image. The default wallpaper is the James Webb Space Telescope's real photo of the
"Cosmic Cliffs" in the Carina Nebula ([credits](assets/wallpapers/CREDITS.md)). Set
`wallpaper = "none"` for Galaxia's built-in deep-space backdrop.

## Dev scripts
| Script | What it does |
|---|---|
| `scripts/run-nested.sh` | Build and run Galaxia in a window |
| `scripts/check.sh` | Gentle `cargo check` (2 jobs, low priority) |
| `scripts/test-shot.sh` | Run with test windows and save a Galaxia-only screenshot to `target/shots/` |

## License
GPL-3.0-or-later
