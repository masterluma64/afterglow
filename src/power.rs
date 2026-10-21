//! Keeps Windows from sleeping or turning off the display while a video plays.

const ES_CONTINUOUS: u32 = 0x8000_0000;
const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetThreadExecutionState(flags: u32) -> u32;
}

/// The request is tied to the calling thread, so always use it from the UI
/// thread.
#[derive(Default)]
pub struct SleepBlocker {
    blocking: bool,
}

impl SleepBlocker {
    pub fn set(&mut self, block: bool) {
        if block == self.blocking {
            return;
        }
        let flags = if block { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
        // Returns 0 on failure; there's nothing better to do than try again later.
        if unsafe { SetThreadExecutionState(flags) } != 0 {
            self.blocking = block;
        }
    }
}
