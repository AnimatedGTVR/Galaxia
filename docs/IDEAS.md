# DreamDM: ideas

Name (current favorite): **Galaxia**. Folder is still dreamdm.

## Already decided
- Built for me to use every day, not to get famous
- Rust + Smithay, Shell for scripts
- Mix of tiling and floating: scrollable tiling like niri (columns on an endless horizontal strip), plus floating windows
- Looks: CaliDE / COSMIC. Feel: PancakeDM
- Transparent frosted glass is a core part of the look
- Must NOT feel like a copy of GNOME or Hyprland

## Name ideas
- Galaxia (current pick)
- Backups: Lucid, Ganymede, Galacia

## Feature ideas
### Scrollable tiling + floating
- New windows open as a new column to the right, so nothing ever squishes
- Each column can hold stacked windows; resize width with presets (1/3, 1/2, 2/3, full)
- Workspaces stack vertically, and each one has its own strip
- One key floats or unfloats a window; floating windows sit on top of the strip
- Our own twists so it isn't a niri copy:
  - Minimap of the strip on the panel, so you can see and click where you are
  - Pin a floating window so it follows you as you scroll
  - Saved strips ("Coding", "Gaming") that bring back the same columns
  - Touchpad: smooth swipe to scroll, pinch to zoom out to an overview

### Glass look
- Frosted glass (blurred and see-through) on the panel, launcher, notifications, menus, and overview
- Windows can opt in to glass, set per app or with a key
- Glass gets a slight tint from the accent color plus a thin bright edge, so it reads as glass and not mush
- One slider for blur strength and one for transparency
- Performance: blur only what changed, and drop to plain see-through when on battery or when a fullscreen game is running
- Readability guard: if text over the glass gets hard to read, add more tint automatically

### Riced COSMIC look (main inspiration)
- Floating panel and dock with gaps from the screen edges, not stuck to them
- Big rounded corners everywhere
- Gaps between tiled windows, with a soft shadow on each window
- Glass panels with strong accent colors
- Clean, roomy spacing with no clutter
- Drop screenshots that show the look into inspo/

### Shell inspiration: DankMaterialShell, Caelestia, and similar
- Shell = everything around the windows: bar, launcher, control center, notifications, widgets
- Liked: rich animated control center, media and calendar widgets, colors from the wallpaper, smooth popups
- DECIDED: no Quickshell. The shell is our own, written in Rust
