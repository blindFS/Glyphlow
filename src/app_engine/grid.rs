use super::AppEngine;
use crate::{
    Mode,
    ax_element::ElementOfInterest,
    config::{GridConfig, RoleOfInterest},
    user_interface::{GridOverlay, cell_frame, parent_frame},
    util::Frame,
};

/// Where a recursive-grid run currently is.
#[derive(Clone, Copy)]
struct GridLevel {
    /// The region the recursion started from, which every level stays aligned to.
    outer: Frame,
    /// The region still being narrowed down.
    region: Frame,
    /// How many levels down `region` is; zero while it is still `outer`.
    depth: usize,
}

impl GridLevel {
    /// The level the cell labelled `key` opens, or `None` if no cell carries it.
    fn narrowed(&self, key: char, grid: &GridConfig) -> Option<Self> {
        let idx = grid.label_index(key)?;
        let (rows, cols) = grid.dims();
        Some(Self {
            region: cell_frame(&self.region, rows, cols, idx),
            depth: self.depth + 1,
            ..*self
        })
    }

    /// The level above, or `None` once the run is back at `outer`.
    fn widened(&self, grid: &GridConfig) -> Option<Self> {
        let (rows, cols) = grid.dims();
        Some(Self {
            region: parent_frame(&self.region, &self.outer, rows, cols),
            depth: self.depth.checked_sub(1)?,
            ..*self
        })
    }
}

/// A live recursive-grid run.
pub(crate) struct GridSession {
    level: GridLevel,
    overlay: GridOverlay,
    /// The mode to go back to once the grid closes.
    prev_mode: Mode,
}

impl AppEngine {
    /// The region the grid subdivides: the selected window, or the focused one
    /// when nothing is selected.
    fn grid_region(&self) -> Frame {
        self.selected
            .as_ref()
            .filter(|s| s.role() == RoleOfInterest::Window)
            .map(|s| s.frame)
            .unwrap_or(self.last_app_window_info.frame)
    }

    /// Put the grid up over the window and wait for cell keys.
    pub(super) fn start_grid(&mut self) {
        let prev_mode = self.mode();
        self.drawer.clear_menus();
        self.clear_grid();

        // The first level is the whole region, so it is its own `outer`.
        let region = self.grid_region();
        let grid = &self.config.grid;
        let overlay = GridOverlay::draw(
            &region,
            &grid.labels(),
            grid,
            &self.drawer.root,
            &self.overlay_frame,
        );

        self.grid = Some(GridSession {
            level: GridLevel {
                outer: region,
                region,
                depth: 0,
            },
            overlay,
            prev_mode,
        });
        self.set_mode(Mode::Grid);
    }

    /// Narrow the grid to the cell `key` labels, moving the cursor there.
    pub(super) fn grid_step(&mut self, key: char) {
        let Some(level) = self
            .grid
            .as_ref()
            .and_then(|s| s.level.narrowed(key, &self.config.grid))
        else {
            return;
        };
        self.show_grid(level);
    }

    /// Widen the grid back to the level above, moving the cursor there. Nothing
    /// sits above the region the recursion started from, so that is a no-op.
    pub(super) fn grid_back(&mut self) {
        let Some(level) = self
            .grid
            .as_ref()
            .and_then(|s| s.level.widened(&self.config.grid))
        else {
            return;
        };
        self.show_grid(level);
    }

    /// Move the cursor to `level` and redraw the cells over it.
    fn show_grid(&mut self, level: GridLevel) {
        let (x, y) = level.region.center();
        self.move_mouse_with_trail(x, y);

        let Some(session) = self.grid.as_mut() else {
            return;
        };
        session
            .overlay
            .place(&level.region, &self.config.grid, &self.overlay_frame);
        session.level = level;
    }

    /// Keep the cell the user settled on and run the rest of the workflow.
    pub(super) fn accept_grid(&mut self) {
        let Some(GridSession {
            level,
            overlay,
            prev_mode,
        }) = self.grid.take()
        else {
            return;
        };
        overlay.free();

        // Element-relative actions (`Click`) need a frame to aim at.
        let (x, y) = level.region.center();
        self.selected = Some(ElementOfInterest::pseudo(
            None,
            Frame::new(x - 1.0, y - 1.0, x + 1.0, y + 1.0),
        ));
        self.set_mode(prev_mode);
        self.execute_pending_workflow_actions();
    }

    pub(super) fn clear_grid(&mut self) {
        if let Some(session) = self.grid.take() {
            session.overlay.free();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 900 px window: the 3x3 default divides it exactly, twice over, so the
    /// regions below compare exactly instead of within a tolerance.
    fn window_and_grid() -> (GridConfig, GridLevel) {
        let region = Frame::new(100.0, 200.0, 1000.0, 1100.0);
        (
            GridConfig::default(),
            GridLevel {
                outer: region,
                region,
                depth: 0,
            },
        )
    }

    /// Widening undoes narrowing even though the parent is recomputed, not popped:
    /// the levels stay aligned to the window because `outer` is never rebound.
    #[test]
    fn widening_undoes_narrowing_back_to_the_window() {
        let (grid, root) = window_and_grid();

        // Two levels down, so widening rebuilds a parent that is not the window.
        let once = root
            .narrowed('K', &grid)
            .expect("`K` labels the middle cell");
        let twice = once
            .narrowed('U', &grid)
            .expect("`U` labels the first cell");
        assert_eq!(
            (twice.region, twice.depth),
            (Frame::new(400.0, 500.0, 500.0, 600.0), 2)
        );
        assert_eq!(twice.outer, root.outer, "a level never rebinds `outer`");

        let back = twice
            .widened(&grid)
            .expect("the middle cell is one level up");
        assert_eq!((back.region, back.depth), (once.region, 1));

        let out = back.widened(&grid).expect("the window is still a level");
        assert_eq!((out.region, out.depth), (root.region, 0));

        // Nothing sits above the window, so Backspace there is a no-op.
        assert!(out.widened(&grid).is_none());
    }

    /// A key that names no cell leaves the run where it is.
    #[test]
    fn narrowing_ignores_a_key_that_labels_no_cell() {
        let (grid, root) = window_and_grid();

        assert!(root.narrowed('Z', &grid).is_none());
        // `/` is the grid's own key; the hint modes reserve it for search.
        assert!(root.narrowed('/', &grid).is_some());
    }
}
