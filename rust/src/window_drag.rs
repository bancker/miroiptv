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
//!
//! egui hears of a button going up from a window message, and in rare cases it
//! never does (another window took the mouse capture; the release came while
//! egui did not know where the pointer was). A drag that had only egui to go by
//! would then stay glued to the cursor. So while a drag runs, and only then, it
//! also asks Windows whether the button is still down (`GetAsyncKeyState`, bit
//! 15 only). That call reports the PHYSICAL buttons, and egui's right button is
//! the LOGICAL one: for a left-handed user who swapped them (`SM_SWAPBUTTON`) it
//! is the physical left one. The swap is read once, when the drag begins.

use egui::Pos2;

/// A place on the screen in physical pixels.
pub type ScreenPos = (i32, i32);

/// Virtual-key codes of the physical mouse buttons, for `GetAsyncKeyState`.
pub const VK_LBUTTON: i32 = 0x01;
pub const VK_RBUTTON: i32 = 0x02;

/// The physical button that is egui's right (`Secondary`) one: the left one
/// when the buttons are swapped.
pub fn button_to_poll(swapped: bool) -> i32 {
    if swapped {
        VK_LBUTTON
    } else {
        VK_RBUTTON
    }
}

/// What a drag asks Windows about the mouse. `Mouse::WINDOWS` is the real thing;
/// a test puts its own in the app, to say where the cursor is and which buttons
/// are down.
#[derive(Clone, Copy)]
pub struct Mouse {
    /// Where the cursor is on the screen.
    pub cursor: fn() -> Option<ScreenPos>,
    /// Whether the buttons are swapped (the left-handed setting).
    pub buttons_swapped: fn() -> bool,
    /// Whether the physical button with this virtual-key code is down now.
    pub key_down: fn(i32) -> bool,
}

impl Mouse {
    pub const WINDOWS: Mouse = Mouse {
        cursor: cursor_pos,
        buttons_swapped,
        key_down,
    };
}

/// The few calls into Windows (user32) the drag needs, in one block.
#[cfg(windows)]
mod win32 {
    /// Win32's POINT.
    #[repr(C)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    /// The `GetSystemMetrics` index of the swapped-buttons setting.
    pub const SM_SWAPBUTTON: i32 = 23;

    #[link(name = "user32")]
    extern "system" {
        pub fn GetCursorPos(point: *mut Point) -> i32;
        pub fn GetAsyncKeyState(vkey: i32) -> i16;
        pub fn GetSystemMetrics(index: i32) -> i32;
    }
}

/// Where the cursor is on the screen, in physical pixels; `None` when Windows
/// will not say. winit makes the process DPI aware before it makes a window, so
/// these are the same pixels as the window's own position.
#[cfg(windows)]
pub fn cursor_pos() -> Option<ScreenPos> {
    let mut point = win32::Point { x: 0, y: 0 };
    // SAFETY: `point` is a POINT that outlives the call, which only fills it in.
    let ok = unsafe { win32::GetCursorPos(&mut point) };
    (ok != 0).then_some((point.x, point.y))
}

/// Whether the mouse buttons are swapped (the left-handed setting).
#[cfg(windows)]
pub fn buttons_swapped() -> bool {
    // SAFETY: takes and returns plain integers.
    unsafe { win32::GetSystemMetrics(win32::SM_SWAPBUTTON) != 0 }
}

/// Whether the physical button with this virtual-key code is down now.
#[cfg(windows)]
pub fn key_down(vkey: i32) -> bool {
    // SAFETY: takes and returns plain integers.
    is_down(unsafe { win32::GetAsyncKeyState(vkey) })
}

/// What `GetAsyncKeyState` returned, as "the key is down": bit 15 (the sign bit)
/// says so, and nothing else does. Bit 0 ("pressed since the last call") is not
/// reliable.
pub fn is_down(state: i16) -> bool {
    state < 0
}

#[cfg(not(windows))]
pub fn cursor_pos() -> Option<ScreenPos> {
    None
}

#[cfg(not(windows))]
pub fn buttons_swapped() -> bool {
    false
}

#[cfg(not(windows))]
pub fn key_down(_vkey: i32) -> bool {
    false
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
    /// The physical button that holds the drag, as a virtual-key code.
    button: i32,
}

impl RightDrag {
    /// The button went down with the cursor at `cursor` and the window's outer
    /// corner at `window`, in points of `pixels_per_point` pixels. `button` is
    /// the physical button to watch (`button_to_poll`).
    pub fn begin(cursor: ScreenPos, window: Pos2, pixels_per_point: f32, button: i32) -> Self {
        let window = (
            (window.x * pixels_per_point).round() as i32,
            (window.y * pixels_per_point).round() as i32,
        );
        Self {
            cursor,
            window,
            sent: window,
            button,
        }
    }

    /// The physical button that holds the drag, as a virtual-key code.
    pub fn button(&self) -> i32 {
        self.button
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
        let mut drag = RightDrag::begin((500, 400), pos2(100.0, 80.0), 1.0, VK_RBUTTON);
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
        let mut drag = RightDrag::begin((500, 400), pos2(100.0, 80.0), 1.5, VK_RBUTTON);
        // 150 and 75 pixels are 100 and 50 points.
        assert_eq!(drag.follow((650, 475), 1.5), Some(pos2(200.0, 130.0)));
        // 30 pixels left and 15 up of where it began: 20 and 10 points.
        assert_eq!(drag.follow((470, 385), 1.5), Some(pos2(80.0, 70.0)));
    }

    #[test]
    fn a_cursor_that_has_not_moved_sends_nothing() {
        let mut drag = RightDrag::begin((500, 400), pos2(100.0, 80.0), 1.0, VK_RBUTTON);
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
    fn the_window_comes_back_to_the_pixel_it_started_at_whatever_the_scale() {
        // egui hands the window's place over as pixels / scale in f32, and winit
        // turns what we send back into pixels with round(scale * point). A scale
        // that is not exactly representable (1.1, 1.25) must not lose a pixel on
        // the way there and back, or the window would creep with every drag.
        for scale in [1.0_f32, 1.1, 1.25, 1.375, 1.5, 1.75, 2.0, 3.0] {
            for px in -4000..4000 {
                let py = px / 2 + 7;
                for (dx, dy) in [(-37, 11), (100, 50)] {
                    let window = pos2(px as f32 / scale, py as f32 / scale);
                    let mut drag = RightDrag::begin((500, 400), window, scale, VK_RBUTTON);
                    let to = drag
                        .follow((500 + dx, 400 + dy), scale)
                        .expect("the cursor moved");
                    let pixels = ((to.x * scale).round() as i32, (to.y * scale).round() as i32);
                    assert_eq!(
                        pixels,
                        (px + dx, py + dy),
                        "scale {scale}, window pixel ({px}, {py}), cursor moved ({dx}, {dy})"
                    );
                }
            }
        }
    }

    #[test]
    fn a_key_is_down_when_bit_15_of_its_state_is_set_and_only_then() {
        assert!(is_down(0x8000_u16 as i16));
        assert!(
            is_down(0x8001_u16 as i16),
            "down, and pressed since the last call"
        );
        assert!(
            !is_down(0x0001),
            "bit 0 alone: pressed since the last call, up now"
        );
        assert!(!is_down(0x7FFF), "every bit but 15");
        assert!(!is_down(0));
    }

    #[test]
    fn the_logical_right_button_is_the_physical_left_one_when_the_buttons_are_swapped() {
        assert_eq!(button_to_poll(false), VK_RBUTTON);
        assert_eq!(button_to_poll(true), VK_LBUTTON);
    }

    #[test]
    fn a_change_of_scale_in_mid_drag_does_not_make_the_window_jump() {
        // Pressed at one pixel to a point with the window at pixel (100, 80); then
        // the screen's scale becomes 2 (the window crossed to another monitor, or
        // the UI zoom changed). 100 pixels right of where the cursor was, the
        // window belongs at pixel (200, 80), which is point (100, 40) now.
        let mut drag = RightDrag::begin((500, 400), pos2(100.0, 80.0), 1.0, VK_RBUTTON);
        assert_eq!(drag.follow((600, 400), 2.0), Some(pos2(100.0, 40.0)));
    }
}
