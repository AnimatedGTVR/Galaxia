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
const DEFAULT_WIDTH: f32 = 0.5;

#[derive(Debug, Clone)]
pub struct Column {
    pub windows: Vec<WindowId>,
    pub width: f32, // fraction of output width
    pub focused: usize,
    /// Width to go back to when leaving full width.
    restore_width: Option<f32>,
}

impl Column {
    fn new(id: WindowId) -> Self {
        Column { windows: vec![id], width: DEFAULT_WIDTH, focused: 0, restore_width: None }
    }
}

#[derive(Debug, Clone)]
pub struct Floating {
    pub id: WindowId,
    /// Position relative to the usable area of the screen.
    pub rect: Rect,
    /// Shown on every workspace.
    pub pinned: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Workspace {
    pub columns: Vec<Column>,
    pub floating: Vec<Floating>,
    pub focused: usize,
    /// Target strip x offset in pixels (the compositor animates towards it).
    pub scroll: f32,
}

impl Workspace {
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty() && self.floating.is_empty()
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.is_floating(id) || self.columns.iter().any(|c| c.windows.contains(&id))
    }

    pub fn is_floating(&self, id: WindowId) -> bool {
        self.floating.iter().any(|f| f.id == id)
    }

    /// A new window opens as a new column right of the focused one.
    pub fn open(&mut self, id: WindowId) {
        let at = if self.columns.is_empty() { 0 } else { self.focused + 1 };
        self.columns.insert(at, Column::new(id));
        self.focused = at;
    }

    /// Add a window straight to the floating layer.
    pub fn open_floating(&mut self, id: WindowId, rect: Rect, pinned: bool) {
        self.floating.push(Floating { id, rect, pinned });
    }

    /// Remove a window wherever it is. Returns its floating state if it was floating.
    pub fn close(&mut self, id: WindowId) -> Option<Floating> {
        if let Some(i) = self.floating.iter().position(|f| f.id == id) {
            return Some(self.floating.remove(i));
        }
        for c in &mut self.columns {
            if let Some(i) = c.windows.iter().position(|w| *w == id) {
                c.windows.remove(i);
                c.focused = c.focused.min(c.windows.len().saturating_sub(1));
            }
        }
        let before = self.columns.len();
        let removed_before_focus = self.columns[..self.focused.min(before)].iter().filter(|c| c.windows.is_empty()).count();
        self.columns.retain(|c| !c.windows.is_empty());
        self.focused = self.focused.saturating_sub(removed_before_focus).min(self.columns.len().saturating_sub(1));
        None
    }

    /// The tiled window that has focus, if any.
    pub fn focused_window(&self) -> Option<WindowId> {
        let c = self.columns.get(self.focused)?;
        c.windows.get(c.focused).copied()
    }

    /// Focus a tiled window by id (e.g. after a click). Returns false if floating/unknown.
    pub fn focus_window(&mut self, id: WindowId) -> bool {
        for (i, c) in self.columns.iter_mut().enumerate() {
            if let Some(j) = c.windows.iter().position(|w| *w == id) {
                c.focused = j;
                self.focused = i;
                return true;
            }
        }
        false
    }

    pub fn focus_left(&mut self) {
        self.focused = self.focused.saturating_sub(1);
    }

    pub fn focus_right(&mut self) {
        if self.focused + 1 < self.columns.len() {
            self.focused += 1;
        }
    }

    pub fn focus_up(&mut self) {
        if let Some(c) = self.columns.get_mut(self.focused) {
            c.focused = c.focused.saturating_sub(1);
        }
    }

    pub fn focus_down(&mut self) {
        if let Some(c) = self.columns.get_mut(self.focused) {
            if c.focused + 1 < c.windows.len() {
                c.focused += 1;
            }
        }
    }

    pub fn move_column_left(&mut self) {
        if self.focused > 0 {
            self.columns.swap(self.focused, self.focused - 1);
            self.focused -= 1;
        }
    }

    pub fn move_column_right(&mut self) {
        if self.focused + 1 < self.columns.len() {
            self.columns.swap(self.focused, self.focused + 1);
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
        if col.windows.is_empty() {
            self.columns.remove(self.focused);
        }
        self.focused -= 1;
        let left = &mut self.columns[self.focused];
        left.windows.push(id);
        left.focused = left.windows.len() - 1;
    }

    /// Pull the focused window out of a stacked column into its own column on the right.
    pub fn expel_right(&mut self) {
        let Some(col) = self.columns.get_mut(self.focused) else { return };
        if col.windows.len() < 2 {
            return;
        }
        let id = col.windows.remove(col.focused);
        col.focused = col.focused.min(col.windows.len() - 1);
        self.columns.insert(self.focused + 1, Column::new(id));
        self.focused += 1;
    }

    /// Cycle the focused column through the width presets.
    pub fn cycle_width(&mut self) {
        if let Some(c) = self.columns.get_mut(self.focused) {
            let i = WIDTH_PRESETS.iter().position(|p| (p - c.width).abs() < 1e-3);
            c.width = WIDTH_PRESETS[i.map_or(1, |i| (i + 1) % WIDTH_PRESETS.len())];
            c.restore_width = None;
        }
    }

    /// Toggle the focused column between full width and its previous width.
    pub fn toggle_full_width(&mut self) {
        if let Some(c) = self.columns.get_mut(self.focused) {
            match c.restore_width.take() {
                Some(w) => c.width = w,
                None => {
                    c.restore_width = Some(c.width);
                    c.width = 1.0;
                }
            }
        }
    }

    /// Toggle a window between tiled and floating (centered, half the area).
    pub fn toggle_float(&mut self, id: WindowId, area_w: f32, area_h: f32) {
        if self.close(id).is_some() {
            self.open(id);
            return;
        }
        let (w, h) = (area_w * 0.5, area_h * 0.6);
        self.open_floating(id, Rect { x: (area_w - w) / 2.0, y: (area_h - h) / 2.0, w, h }, false);
    }

    pub fn floating_mut(&mut self, id: WindowId) -> Option<&mut Floating> {
        self.floating.iter_mut().find(|f| f.id == id)
    }

    /// Bring a floating window to the top of the floating stack.
    pub fn raise_floating(&mut self, id: WindowId) {
        if let Some(i) = self.floating.iter().position(|f| f.id == id) {
            let f = self.floating.remove(i);
            self.floating.push(f);
        }
    }

    fn column_x(&self, out_w: f32, gap: f32) -> Vec<(f32, f32)> {
        let mut x = gap;
        self.columns
            .iter()
            .map(|c| {
                let w = (c.width * out_w - gap * 2.0).max(1.0);
                let r = (x, w);
                x += w + gap;
                r
            })
            .collect()
    }

    /// Set the scroll target so the focused column is fully visible, moving as little as possible.
    /// A column wider than the screen gets aligned to its left edge.
    pub fn ensure_focused_visible(&mut self, out_w: f32, gap: f32) {
        let Some(&(x, w)) = self.column_x(out_w, gap).get(self.focused) else {
            self.scroll = 0.0;
            return;
        };
        if x - gap < self.scroll || w + gap * 2.0 > out_w {
            self.scroll = x - gap;
        } else if x + w + gap > self.scroll + out_w {
            self.scroll = x + w + gap - out_w;
        }
    }

    /// Rectangles for every tiled window, given the strip offset to draw at.
    pub fn arrange(&self, out_w: f32, out_h: f32, gap: f32, scroll: f32) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        for (c, (x, w)) in self.columns.iter().zip(self.column_x(out_w, gap)) {
            let n = c.windows.len() as f32;
            let h = ((out_h - gap * (n + 1.0)) / n).max(1.0);
            for (j, id) in c.windows.iter().enumerate() {
                let y = gap + j as f32 * (h + gap);
                out.push((*id, Rect { x: x - scroll, y, w, h }));
            }
        }
        out
    }
}

/// Move `current` towards `target` with exponential easing. Returns true while still moving.
pub fn ease(current: &mut f32, target: f32, dt_secs: f32, anim_ms: u32) -> bool {
    if anim_ms == 0 || (target - *current).abs() < 0.5 {
        let moved = *current != target;
        *current = target;
        return moved;
    }
    let tau = anim_ms as f32 / 1000.0 / 4.0;
    *current += (target - *current) * (1.0 - (-dt_secs / tau).exp());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_with(n: u64) -> Workspace {
        let mut ws = Workspace::default();
        for i in 1..=n {
            ws.open(i);
        }
        ws
    }

    fn order(ws: &Workspace) -> Vec<Vec<WindowId>> {
        ws.columns.iter().map(|c| c.windows.clone()).collect()
    }

    #[test]
    fn opens_to_the_right_of_focus() {
        let mut ws = ws_with(2);
        ws.focus_left();
        ws.open(3);
        assert_eq!(order(&ws), vec![vec![1], vec![3], vec![2]]);
        assert_eq!(ws.focused, 1);
    }

    #[test]
    fn close_removes_empty_columns_and_keeps_focus() {
        let mut ws = ws_with(3);
        ws.close(1); // a column left of focus goes away
        assert_eq!(order(&ws), vec![vec![2], vec![3]]);
        assert_eq!(ws.focused_window(), Some(3));
        ws.close(3);
        assert_eq!(ws.focused_window(), Some(2));
    }

    #[test]
    fn consume_and_expel() {
        let mut ws = ws_with(2);
        ws.consume_left();
        assert_eq!(order(&ws), vec![vec![1, 2]]);
        ws.expel_right();
        assert_eq!(order(&ws), vec![vec![1], vec![2]]);
        assert_eq!(ws.focused_window(), Some(2));
    }

    #[test]
    fn focus_up_down_in_column() {
        let mut ws = ws_with(2);
        ws.consume_left();
        ws.focus_up();
        assert_eq!(ws.focused_window(), Some(1));
        ws.focus_down();
        assert_eq!(ws.focused_window(), Some(2));
    }

    #[test]
    fn move_columns() {
        let mut ws = ws_with(3);
        ws.move_column_left();
        assert_eq!(order(&ws), vec![vec![1], vec![3], vec![2]]);
        ws.move_column_right();
        assert_eq!(order(&ws), vec![vec![1], vec![2], vec![3]]);
    }

    #[test]
    fn width_cycles_and_full_width_restores() {
        let mut ws = ws_with(1);
        ws.cycle_width();
        assert!((ws.columns[0].width - 2.0 / 3.0).abs() < 1e-3);
        ws.toggle_full_width();
        assert_eq!(ws.columns[0].width, 1.0);
        ws.toggle_full_width();
        assert!((ws.columns[0].width - 2.0 / 3.0).abs() < 1e-3);
    }

    #[test]
    fn float_round_trip() {
        let mut ws = ws_with(1);
        ws.toggle_float(1, 1920.0, 1080.0);
        assert!(ws.columns.is_empty());
        assert!(ws.is_floating(1));
        ws.toggle_float(1, 1920.0, 1080.0);
        assert_eq!(ws.columns.len(), 1);
        assert!(ws.floating.is_empty());
    }

    #[test]
    fn scrolls_to_keep_focus_visible() {
        let mut ws = ws_with(4); // four half-width columns: wider than the screen
        ws.ensure_focused_visible(1000.0, 10.0);
        assert!(ws.scroll > 0.0);
        let r = ws.arrange(1000.0, 800.0, 10.0, ws.scroll);
        let last = r.iter().find(|(id, _)| *id == 4).unwrap().1;
        assert!(last.x >= 0.0 && last.x + last.w <= 1000.0);
    }

    #[test]
    fn ease_reaches_target() {
        let mut v = 0.0;
        for _ in 0..200 {
            ease(&mut v, 100.0, 1.0 / 60.0, 220);
        }
        assert_eq!(v, 100.0);
        let mut w = 5.0;
        assert!(ease(&mut w, 50.0, 0.016, 0));
        assert_eq!(w, 50.0);
    }
}
