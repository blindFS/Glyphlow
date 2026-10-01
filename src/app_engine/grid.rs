use super::AppEngine;
use crate::{
    Mode,
    ax_element::ElementOfInterest,
    config::RoleOfInterest,
    user_interface::{GridOverlay, cell_frame},
    util::Frame,
};

/// A live recursive-grid session.
pub(crate) struct GridSession {
    /// The region still being narrowed down.
    region: Frame,
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
        self.draw_grid(self.grid_region(), prev_mode);
        self.set_mode(Mode::Grid);
    }

    /// Narrow the grid to the cell `key` labels, moving the cursor there.
    pub(super) fn grid_step(&mut self, key: char) {
        let Some((region, prev_mode)) = self.grid.as_ref().map(|s| (s.region, s.prev_mode.clone()))
        else {
            return;
        };
        let labels = self.config.grid.labels();
        let Some(idx) = labels.iter().position(|c| *c == key) else {
            return;
        };

        let (rows, cols) = self.config.grid.dims();
        let cell = cell_frame(&region, rows, cols, idx);
        let (x, y) = cell.center();
        self.move_mouse_with_trail(x, y);
        self.draw_grid(cell, prev_mode);
    }

    /// Close the grid where the user left it and run the rest of the workflow.
    pub(super) fn finish_grid(&mut self) {
        let Some(GridSession {
            region,
            overlay,
            prev_mode,
        }) = self.grid.take()
        else {
            return;
        };
        overlay.free();

        // Element-relative actions (`Click`) need a frame to aim at.
        let (x, y) = region.center();
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

    fn draw_grid(&mut self, region: Frame, prev_mode: Mode) {
        self.clear_grid();
        let grid = &self.config.grid;
        let overlay = GridOverlay::draw(
            &region,
            &grid.labels(),
            grid,
            &self.drawer.root,
            &self.overlay_frame,
        );
        self.grid = Some(GridSession {
            region,
            overlay,
            prev_mode,
        });
    }
}
