//! Checks a saved window position against the monitors connected now, so a
//! window last seen on a monitor that's gone doesn't open off-screen, and
//! finds the free part of the screen a window is on.

use crate::library::progress::WindowPlacement;

const MONITOR_DEFAULTTONULL: u32 = 0;
const MONITOR_DEFAULTTONEAREST: u32 = 2;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn MonitorFromPoint(point: Point, flags: u32) -> isize;
    fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
    fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
}

/// The part of the screen the window `hwnd` is on that the taskbar leaves
/// free, as left, top, right and bottom in pixels.
pub fn work_area(hwnd: isize) -> Option<[i32; 4]> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MonitorInfo { size: size_of::<MonitorInfo>() as u32, ..Default::default() };
    if monitor == 0 || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let Rect { left, top, right, bottom } = info.work;
    Some([left, top, right, bottom])
}

/// Whether the window's title bar would be on a monitor: a little in from its
/// top-left corner, where it can be grabbed and moved.
pub fn title_bar_visible(window: &WindowPlacement) -> bool {
    let at = |x: f32, y: f32| Point { x: ((window.x + x) * window.scale) as i32, y: ((window.y + y) * window.scale) as i32 };
    let on_screen = |p: Point| unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONULL) } != 0;
    on_screen(at(60.0, 12.0)) || on_screen(at(window.width - 60.0, 12.0))
}
