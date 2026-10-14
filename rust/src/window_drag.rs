//! Moving the borderless window with the right mouse button. The `t` key takes
//! the title bar away, and a window without one cannot be grabbed: the right
//! button holds it instead, as it did in the C player.
//!
//! The window goes where the cursor's place on the SCREEN says: when the button
//! goes down the cursor and the window are noted, from then on the window sits
//! at its noted place plus how far the cursor has moved since. The cursor's
//! place in the window is no use: it shifts as the window moves, and the window
//! lags and jitters behind it.
//!
//! All of it is kept in physical pixels, as Windows gives them. Points, egui's
//! unit, come in at the end, at the scale of that very frame: a window dragged
//! onto a screen of another scale, or a UI zoom changed meanwhile, does not make
//! it jump.

use egui::Pos2;

/// A place on the screen in physical pixels.
pub type ScreenPos = (i32, i32);

/// Where the cursor is on the screen, in physical pixels; `None` when Windows
/// will not say. winit makes the process DPI aware before it makes a window, so
/// these are the same pixels as the window's own position.
#[cfg(windows)]
pub fn cursor_pos() -> Option<ScreenPos> {
    /// Win32's POINT.
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(point: *mut Point) -> i32;
    }
    let mut point = Point { x: 0, y: 0 };
    // SAFETY: `point` is a POINT that outlives the call, which only fills it in.
    let ok = unsafe { GetCursorPos(&mut point) };
    (ok != 0).then_some((point.x, point.y))
}

#[cfg(not(windows))]
pub fn cursor_pos() -> Option<ScreenPos> {
    None
}

/// A drag of the window in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RightDrag {
    /// The cursor and the window's outer corner when the button went down.
    cursor: ScreenPos,
    window: ScreenPos,
    /// Where the window was sent last: it is sent again only when the cursor
    /// has taken it somewhere else.
    sent: ScreenPos,
}

impl RightDrag {
    /// The button went down with the cursor at `cursor` and the window's outer
    /// corner at `window`, in points of `pixels_per_point` pixels.
    pub fn begin(cursor: ScreenPos, window: Pos2, pixels_per_point: f32) -> Self {
        let window = (
            (window.x * pixels_per_point).round() as i32,
            (window.y * pixels_per_point).round() as i32,
        );
        Self {
            cursor,
            window,
            sent: window,
        }
    }

    /// The cursor is at `cursor` now: where the window belongs, in points of
    /// `pixels_per_point` pixels, or `None` when it was sent there already.
    pub fn follow(&mut self, cursor: ScreenPos, pixels_per_point: f32) -> Option<Pos2> {
        let target = (
            self.window.0 + cursor.0 - self.cursor.0,
            self.window.1 + cursor.1 - self.cursor.1,
        );
        if target == self.sent {
            return None;
        }
        self.sent = target;
        Some(Pos2::new(
            target.0 as f32 / pixels_per_point,
            target.1 as f32 / pixels_per_point,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn the_window_goes_where_it_was_plus_how_far_the_cursor_moved() {
        // The window's corner at (100, 80) points, the cursor at pixel (500, 400)
        // when the button goes down, one pixel to a point.
        let mut drag = RightDrag::begin((500, 400), pos2(100.0, 80.0), 1.0);
        assert_eq!(
            drag.follow((600, 450), 1.0),
            Some(pos2(200.0, 130.0)),
            "right and down"
        );
        assert_eq!(
            drag.follow((450, 380), 1.0),
            Some(pos2(50.0, 60.0)),
            "left and up, past where it began"
        );
    }

    #[test]
    fn the_cursors_pixels_are_points_at_the_scale_of_the_screen() {
        // At 1.5 pixels to a point the corner (100, 80) points is at pixel (150, 120).
        let mut drag = RightDrag::begin((500, 400), pos2(100.0, 80.0), 1.5);
        // 150 and 75 pixels are 100 and 50 points.
        assert_eq!(drag.follow((650, 475), 1.5), Some(pos2(200.0, 130.0)));
        // 30 pixels left and 15 up of where it began: 20 and 10 points.
        assert_eq!(drag.follow((470, 385), 1.5), Some(pos2(80.0, 70.0)));
    }

    #[test]
    fn a_cursor_that_has_not_moved_sends_nothing() {
        let mut drag = RightDrag::begin((500, 400), pos2(100.0, 80.0), 1.0);
        assert_eq!(
            drag.follow((500, 400), 1.0),
            None,
            "still where the button went down"
        );
        assert_eq!(drag.follow((520, 400), 1.0), Some(pos2(120.0, 80.0)));
        assert_eq!(drag.follow((520, 400), 1.0), None, "the window is there");
        assert_eq!(
            drag.follow((500, 400), 1.0),
            Some(pos2(100.0, 80.0)),
            "back where it began is a move too"
        );
    }

    #[test]
    fn a_change_of_scale_in_mid_drag_does_not_make_the_window_jump() {
        // Pressed at one pixel to a point with the window at pixel (100, 80); then
        // the screen's scale becomes 2 (the window crossed to another monitor, or
        // the UI zoom changed). 100 pixels right of where the cursor was, the
        // window belongs at pixel (200, 80), which is point (100, 40) now.
        let mut drag = RightDrag::begin((500, 400), pos2(100.0, 80.0), 1.0);
        assert_eq!(drag.follow((600, 400), 2.0), Some(pos2(100.0, 40.0)));
    }
}
