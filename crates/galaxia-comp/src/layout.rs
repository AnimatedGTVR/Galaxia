//! Scrollable tiling layout: windows live in columns on an endless horizontal
//! strip. Floating windows sit above the strip. Pure logic, no Smithay.

pub type WindowId = u64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Column width presets, as a fraction of the output width.
pub const WIDTH_PRESETS: [f32; 4] = [1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0];

#[derive(Debug, Clone)]
pub struct Column {
    pub windows: Vec<WindowId>,
    pub width: f32, // fraction of output width
    pub focused: usize,
}

#[derive(Debug, Clone)]
pub struct Floating {
    pub id: WindowId,
    pub rect: Rect,
    pub pinned: bool, // stays on screen while the strip scrolls
}

#[derive(Debug, Default)]
pub struct Workspace {
    pub columns: Vec<Column>,
    pub floating: Vec<Floating>,
    pub focused: usize,
    pub scroll: f32, // strip x offset in pixels
}

impl Workspace {
    /// A new window opens as a new column right of the focused one.
    pub fn open(&mut self, id: WindowId) {
        let at = if self.columns.is_empty() { 0 } else { self.focused + 1 };
        self.columns.insert(at, Column { windows: vec![id], width: 0.5, focused: 0 });
        self.focused = at;
    }

    pub fn close(&mut self, id: WindowId) {
        self.floating.retain(|f| f.id != id);
        for c in &mut self.columns {
            if let Some(i) = c.windows.iter().position(|w| *w == id) {
                c.windows.remove(i);
                c.focused = c.focused.min(c.windows.len().saturating_sub(1));
            }
        }
        self.columns.retain(|c| !c.windows.is_empty());
        self.focused = self.focused.min(self.columns.len().saturating_sub(1));
    }

    pub fn focus_left(&mut self) {
        self.focused = self.focused.saturating_sub(1);
    }

    pub fn focus_right(&mut self) {
        if self.focused + 1 < self.columns.len() {
            self.focused += 1;
        }
    }

    /// Move the focused window into the column on its left (stacking).
    pub fn consume_left(&mut self) {
        if self.focused == 0 || self.columns.is_empty() {
            return;
        }
        let col = &mut self.columns[self.focused];
        let id = col.windows.remove(col.focused);
        col.focused = col.focused.min(col.windows.len().saturating_sub(1));
        let empty = col.windows.is_empty();
        if empty {
            self.columns.remove(self.focused);
        }
        self.focused -= 1;
        let left = &mut self.columns[self.focused];
        left.windows.push(id);
        left.focused = left.windows.len() - 1;
    }

    /// Cycle the focused column through the width presets.
    pub fn cycle_width(&mut self) {
        if let Some(c) = self.columns.get_mut(self.focused) {
            let i = WIDTH_PRESETS.iter().position(|p| (p - c.width).abs() < 1e-3);
            c.width = WIDTH_PRESETS[i.map_or(1, |i| (i + 1) % WIDTH_PRESETS.len())];
        }
    }

    /// Toggle the focused tiled window to floating (or a floating one back).
    pub fn toggle_float(&mut self, id: WindowId, out_w: f32, out_h: f32) {
        if let Some(i) = self.floating.iter().position(|f| f.id == id) {
            self.floating.remove(i);
            self.open(id);
            return;
        }
        self.close(id);
        let (w, h) = (out_w * 0.5, out_h * 0.6);
        self.floating.push(Floating {
            id,
            rect: Rect { x: (out_w - w) / 2.0, y: (out_h - h) / 2.0, w, h },
            pinned: false,
        });
    }

    /// Scroll so the focused column is fully visible, moving as little as possible.
    pub fn ensure_focused_visible(&mut self, out_w: f32, gap: f32) {
        let mut x = gap;
        for (i, c) in self.columns.iter().enumerate() {
            let w = c.width * out_w - gap * 2.0;
            if i == self.focused {
                if x - gap < self.scroll {
                    self.scroll = x - gap;
                } else if x + w + gap > self.scroll + out_w {
                    self.scroll = x + w + gap - out_w;
                }
                return;
            }
            x += w + gap;
        }
    }

    /// Screen rectangles for every tiled window, after scrolling.
    pub fn arrange(&self, out_w: f32, out_h: f32, gap: f32) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        let mut x = gap;
        for c in &self.columns {
            let w = c.width * out_w - gap * 2.0;
            let n = c.windows.len() as f32;
            let h = (out_h - gap * (n + 1.0)) / n;
            for (j, id) in c.windows.iter().enumerate() {
                let y = gap + j as f32 * (h + gap);
                out.push((*id, Rect { x: x - self.scroll, y, w, h }));
            }
            x += w + gap;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_to_the_right_of_focus() {
        let mut ws = Workspace::default();
        ws.open(1);
        ws.open(2);
        ws.focus_left();
        ws.open(3);
        let ids: Vec<_> = ws.columns.iter().map(|c| c.windows[0]).collect();
        assert_eq!(ids, vec![1, 3, 2]);
        assert_eq!(ws.focused, 1);
    }

    #[test]
    fn close_removes_empty_columns() {
        let mut ws = Workspace::default();
        ws.open(1);
        ws.open(2);
        ws.close(2);
        assert_eq!(ws.columns.len(), 1);
        assert_eq!(ws.focused, 0);
    }

    #[test]
    fn consume_stacks_into_left_column() {
        let mut ws = Workspace::default();
        ws.open(1);
        ws.open(2);
        ws.consume_left();
        assert_eq!(ws.columns.len(), 1);
        assert_eq!(ws.columns[0].windows, vec![1, 2]);
    }

    #[test]
    fn width_cycles_through_presets() {
        let mut ws = Workspace::default();
        ws.open(1);
        ws.cycle_width();
        assert!((ws.columns[0].width - 2.0 / 3.0).abs() < 1e-3);
    }

    #[test]
    fn float_round_trip() {
        let mut ws = Workspace::default();
        ws.open(1);
        ws.toggle_float(1, 1920.0, 1080.0);
        assert!(ws.columns.is_empty());
        assert_eq!(ws.floating.len(), 1);
        ws.toggle_float(1, 1920.0, 1080.0);
        assert_eq!(ws.columns.len(), 1);
        assert!(ws.floating.is_empty());
    }

    #[test]
    fn scrolls_to_keep_focus_visible() {
        let mut ws = Workspace::default();
        for i in 0..4 {
            ws.open(i); // four half-width columns: wider than the screen
        }
        ws.ensure_focused_visible(1000.0, 10.0);
        assert!(ws.scroll > 0.0);
        let r = ws.arrange(1000.0, 800.0, 10.0);
        let last = r.iter().find(|(id, _)| *id == 3).unwrap().1;
        assert!(last.x + last.w <= 1000.0);
    }
}
