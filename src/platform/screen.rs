//! Checks a saved window position against the monitors connected now, so a
//! window last seen on a monitor that's gone doesn't open off-screen.

use crate::library::progress::WindowPlacement;

const MONITOR_DEFAULTTONULL: u32 = 0;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn MonitorFromPoint(point: Point, flags: u32) -> isize;
}

/// Whether the window's title bar would be on a monitor: a little in from its
/// top-left corner, where it can be grabbed and moved.
pub fn title_bar_visible(window: &WindowPlacement) -> bool {
    let at = |x: f32, y: f32| Point { x: ((window.x + x) * window.scale) as i32, y: ((window.y + y) * window.scale) as i32 };
    let on_screen = |p: Point| unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONULL) } != 0;
    on_screen(at(60.0, 12.0)) || on_screen(at(window.width - 60.0, 12.0))
}
