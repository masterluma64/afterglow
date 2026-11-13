//! The playing video's progress on Afterglow's taskbar button: filling up in
//! green while it plays, yellow while it is paused.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Shell::{ITaskbarList3, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED, TBPFLAG, TaskbarList};

/// Steps the bar is shown in: finer than a taskbar button has pixels.
const STEPS: u64 = 1000;

/// Use it from the UI thread, which owns the window.
pub struct TaskbarProgress {
    list: ITaskbarList3,
    hwnd: HWND,
    /// What was last shown: the state, and how far, in `STEPS`.
    shown: Option<(TBPFLAG, u64)>,
}

impl TaskbarProgress {
    pub fn new(hwnd: isize) -> Result<Self, String> {
        unsafe {
            // The UI thread has COM already (for drag and drop); this is in
            // case it doesn't, and is never undone, as the list lives as long.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let list: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
            list.HrInit().map_err(|e| e.message())?;
            Ok(Self { list, hwnd: HWND(hwnd as *mut _), shown: None })
        }
    }

    /// Shows how far into the video playback is (0 to 1), or no bar for
    /// `None`. Only talks to the taskbar when what it shows changes.
    pub fn show(&mut self, progress: Option<f64>, paused: bool) {
        let state = match progress {
            None => TBPF_NOPROGRESS,
            Some(_) if paused => TBPF_PAUSED,
            Some(_) => TBPF_NORMAL,
        };
        let value = progress.map_or(0, |p| (p.clamp(0.0, 1.0) * STEPS as f64).round() as u64);
        if self.shown == Some((state, value)) {
            return;
        }
        // Setting a value turns the bar on in the normal state, so the state
        // goes after it.
        let set_value = || if progress.is_some() { unsafe { self.list.SetProgressValue(self.hwnd, value, STEPS) } } else { Ok(()) };
        let result = set_value().and_then(|()| unsafe { self.list.SetProgressState(self.hwnd, state) });
        // Until the taskbar button exists, this fails; it is tried again
        // on the next change.
        if result.is_ok() {
            self.shown = Some((state, value));
        }
    }
}
