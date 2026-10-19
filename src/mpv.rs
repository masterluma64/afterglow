//! Minimal libmpv binding, loaded at runtime from `libmpv-2.dll`.
//!
//! Video is rendered with mpv's software render API on a dedicated thread and
//! handed to the UI as an `egui::ColorImage`.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use eframe::egui::{self, Color32, ColorImage};
use libloading::Library;

const MPV_FORMAT_INT64: c_int = 4;
const MPV_FORMAT_DOUBLE: c_int = 5;

const MPV_EVENT_NONE: c_int = 0;
const MPV_EVENT_END_FILE: c_int = 7;
const MPV_EVENT_FILE_LOADED: c_int = 8;
const MPV_END_FILE_REASON_EOF: c_int = 0;
const MPV_END_FILE_REASON_ERROR: c_int = 4;

const MPV_RENDER_PARAM_INVALID: c_int = 0;
const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
const MPV_RENDER_PARAM_SW_SIZE: c_int = 17;
const MPV_RENDER_PARAM_SW_FORMAT: c_int = 18;
const MPV_RENDER_PARAM_SW_STRIDE: c_int = 19;
const MPV_RENDER_PARAM_SW_POINTER: c_int = 20;
const MPV_RENDER_UPDATE_FRAME: u64 = 1;

#[repr(C)]
struct RawEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct RawEndFile {
    reason: c_int,
    error: c_int,
}

#[repr(C)]
struct RenderParam {
    ty: c_int,
    data: *mut c_void,
}

type Handle = *mut c_void;
type UpdateFn = unsafe extern "C" fn(*mut c_void);

struct Api {
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(Handle),
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    command: unsafe extern "C" fn(Handle, *mut *const c_char) -> c_int,
    set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property_string: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    free: unsafe extern "C" fn(*mut c_void),
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut RawEvent,
    render_create: unsafe extern "C" fn(*mut *mut c_void, Handle, *mut RenderParam) -> c_int,
    render_set_update_callback: unsafe extern "C" fn(*mut c_void, Option<UpdateFn>, *mut c_void),
    render_update: unsafe extern "C" fn(*mut c_void) -> u64,
    render_render: unsafe extern "C" fn(*mut c_void, *mut RenderParam) -> c_int,
    render_free: unsafe extern "C" fn(*mut c_void),
    _lib: Library,
}

impl Api {
    fn load() -> Result<Self, String> {
        let lib = unsafe { Library::new("libmpv-2.dll") }
            .map_err(|e| format!("Could not load libmpv-2.dll: {e}"))?;
        macro_rules! sym {
            ($name:literal) => {
                *unsafe { lib.get($name) }.map_err(|e| format!("libmpv is missing a symbol: {e}"))?
            };
        }
        Ok(Self {
            create: sym!(b"mpv_create\0"),
            initialize: sym!(b"mpv_initialize\0"),
            terminate_destroy: sym!(b"mpv_terminate_destroy\0"),
            set_option_string: sym!(b"mpv_set_option_string\0"),
            command: sym!(b"mpv_command\0"),
            set_property_string: sym!(b"mpv_set_property_string\0"),
            get_property: sym!(b"mpv_get_property\0"),
            get_property_string: sym!(b"mpv_get_property_string\0"),
            free: sym!(b"mpv_free\0"),
            error_string: sym!(b"mpv_error_string\0"),
            wait_event: sym!(b"mpv_wait_event\0"),
            render_create: sym!(b"mpv_render_context_create\0"),
            render_set_update_callback: sym!(b"mpv_render_context_set_update_callback\0"),
            render_update: sym!(b"mpv_render_context_update\0"),
            render_render: sym!(b"mpv_render_context_render\0"),
            render_free: sym!(b"mpv_render_context_free\0"),
            _lib: lib,
        })
    }

    fn check(&self, code: c_int) -> Result<(), String> {
        if code >= 0 {
            return Ok(());
        }
        let msg = unsafe { CStr::from_ptr((self.error_string)(code)) };
        Err(msg.to_string_lossy().into_owned())
    }
}

/// Events the app cares about.
pub enum Event {
    FileLoaded,
    /// Playback of a file ended. `eof` is true when it played to the end.
    EndFile { eof: bool, error: bool },
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: i64,
    pub label: String,
    pub selected: bool,
}

/// Send/Sync wrapper for raw pointers whose API is documented as thread-safe.
#[derive(Clone, Copy)]
struct Ptr(*mut c_void);
unsafe impl Send for Ptr {}
unsafe impl Sync for Ptr {}

#[derive(Default)]
struct RenderState {
    dirty: bool,
    quit: bool,
    size: [i32; 2],
}

#[derive(Default)]
struct RenderShared {
    state: Mutex<RenderState>,
    cv: Condvar,
    frame: Mutex<Option<ColorImage>>,
}

impl RenderShared {
    fn poke(&self, f: impl FnOnce(&mut RenderState)) {
        let mut st = self.state.lock().unwrap();
        f(&mut st);
        st.dirty = true;
        self.cv.notify_one();
    }
}

unsafe extern "C" fn on_render_update(ctx: *mut c_void) {
    // Called from mpv's threads; must not call back into mpv.
    let shared = unsafe { &*(ctx as *const RenderShared) };
    shared.poke(|_| {});
}

pub struct Mpv {
    api: Arc<Api>,
    handle: Ptr,
    render_ctx: Ptr,
    shared: Arc<RenderShared>,
    render_thread: Option<JoinHandle<()>>,
}

impl Mpv {
    pub fn new(egui_ctx: egui::Context) -> Result<Self, String> {
        let api = Arc::new(Api::load()?);
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("mpv_create failed".into());
        }

        let options = [
            ("vo", "libmpv"),
            ("keep-open", "no"),
            ("idle", "yes"),
            ("input-default-bindings", "no"),
            ("osd-level", "0"),
            ("sub-auto", "fuzzy"),
            ("slang", "en,eng"),
            ("volume-max", "150"),
            ("ytdl", "no"),
        ];
        for (k, v) in options {
            let (k, v) = (cstr(k), cstr(v));
            unsafe { (api.set_option_string)(handle, k.as_ptr(), v.as_ptr()) };
        }
        if let Err(e) = api.check(unsafe { (api.initialize)(handle) }) {
            unsafe { (api.terminate_destroy)(handle) };
            return Err(format!("mpv_initialize failed: {e}"));
        }

        let sw = cstr("sw");
        let mut params = [
            RenderParam { ty: MPV_RENDER_PARAM_API_TYPE, data: sw.as_ptr() as *mut c_void },
            RenderParam { ty: MPV_RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        let mut render_ctx = std::ptr::null_mut();
        let code = unsafe { (api.render_create)(&mut render_ctx, handle, params.as_mut_ptr()) };
        if let Err(e) = api.check(code) {
            unsafe { (api.terminate_destroy)(handle) };
            return Err(format!("mpv_render_context_create failed: {e}"));
        }

        let shared = Arc::new(RenderShared::default());
        unsafe {
            (api.render_set_update_callback)(
                render_ctx,
                Some(on_render_update),
                Arc::as_ptr(&shared) as *mut c_void,
            )
        };

        let render_thread = {
            let api = api.clone();
            let shared = shared.clone();
            let render_ctx = Ptr(render_ctx);
            std::thread::Builder::new()
                .name("mpv-render".into())
                .spawn(move || render_loop(&api, render_ctx, &shared, &egui_ctx))
                .map_err(|e| e.to_string())?
        };

        Ok(Self {
            api,
            handle: Ptr(handle),
            render_ctx: Ptr(render_ctx),
            shared,
            render_thread: Some(render_thread),
        })
    }

    /// Size in physical pixels the video should be rendered at.
    pub fn set_render_size(&self, w: u32, h: u32) {
        let size = [w as i32, h as i32];
        if self.shared.state.lock().unwrap().size != size {
            self.shared.poke(|st| st.size = size);
        }
    }

    /// Latest rendered frame, if a new one is available.
    pub fn take_frame(&self) -> Option<ColorImage> {
        self.shared.frame.lock().unwrap().take()
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        self.api.check(unsafe { (self.api.command)(self.handle.0, ptrs.as_mut_ptr()) })
    }

    pub fn set(&self, name: &str, value: &str) {
        let (n, v) = (cstr(name), cstr(value));
        unsafe { (self.api.set_property_string)(self.handle.0, n.as_ptr(), v.as_ptr()) };
    }

    pub fn get_str(&self, name: &str) -> Option<String> {
        let n = cstr(name);
        let p = unsafe { (self.api.get_property_string)(self.handle.0, n.as_ptr()) };
        if p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { (self.api.free)(p as *mut c_void) };
        Some(s)
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let n = cstr(name);
        let mut v = 0f64;
        let code = unsafe {
            (self.api.get_property)(self.handle.0, n.as_ptr(), MPV_FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void)
        };
        (code >= 0).then_some(v)
    }

    pub fn get_i64(&self, name: &str) -> Option<i64> {
        let n = cstr(name);
        let mut v = 0i64;
        let code = unsafe {
            (self.api.get_property)(self.handle.0, n.as_ptr(), MPV_FORMAT_INT64, &mut v as *mut i64 as *mut c_void)
        };
        (code >= 0).then_some(v)
    }

    pub fn get_flag(&self, name: &str) -> bool {
        self.get_str(name).as_deref() == Some("yes")
    }

    pub fn load_file(&self, path: &Path, start: f64) -> Result<(), String> {
        // `start` is a global option, so it has to be reset for every file.
        self.set("start", &if start > 0.0 { format!("{start:.3}") } else { "none".into() });
        self.command(&["loadfile", &path.to_string_lossy(), "replace"])
    }

    pub fn tracks(&self, kind: &str) -> Vec<Track> {
        let count = self.get_i64("track-list/count").unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let p = |f: &str| self.get_str(&format!("track-list/{i}/{f}"));
                let t = p("type")?;
                if t != kind {
                    return None;
                }
                let id = p("id")?.parse().ok()?;
                let mut label = p("title").unwrap_or_default();
                if let Some(lang) = p("lang") {
                    label = if label.is_empty() { lang } else { format!("{label} [{lang}]") };
                }
                if label.is_empty() {
                    label = format!("Track {id}");
                }
                if p("external").as_deref() == Some("yes") {
                    label.push_str(" (external)");
                }
                Some(Track { id, label, selected: p("selected").as_deref() == Some("yes") })
            })
            .collect()
    }

    pub fn poll_events(&self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            let ev = unsafe { &*(self.api.wait_event)(self.handle.0, 0.0) };
            match ev.event_id {
                MPV_EVENT_NONE => break,
                MPV_EVENT_FILE_LOADED => out.push(Event::FileLoaded),
                MPV_EVENT_END_FILE if !ev.data.is_null() => {
                    let end = unsafe { &*(ev.data as *const RawEndFile) };
                    out.push(Event::EndFile {
                        eof: end.reason == MPV_END_FILE_REASON_EOF,
                        error: end.reason == MPV_END_FILE_REASON_ERROR,
                    });
                }
                _ => {}
            }
        }
        out
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        self.shared.poke(|st| st.quit = true);
        if let Some(t) = self.render_thread.take() {
            let _ = t.join();
        }
        unsafe {
            (self.api.render_set_update_callback)(self.render_ctx.0, None, std::ptr::null_mut());
            (self.api.render_free)(self.render_ctx.0);
            (self.api.terminate_destroy)(self.handle.0);
        }
    }
}

/// 64-byte aligned storage, as recommended for mpv's software renderer.
#[derive(Clone, Copy)]
#[repr(C, align(64))]
struct Block([u8; 64]);

fn render_loop(api: &Api, ctx: Ptr, shared: &RenderShared, egui_ctx: &egui::Context) {
    let mut buf: Vec<Block> = Vec::new();
    let mut last_size = [0, 0];
    let format = cstr("rgb0");

    loop {
        let size = {
            let mut st = shared.state.lock().unwrap();
            while !st.dirty && !st.quit {
                st = shared.cv.wait(st).unwrap();
            }
            if st.quit {
                return;
            }
            st.dirty = false;
            st.size
        };

        let flags = unsafe { (api.render_update)(ctx.0) };
        if flags & MPV_RENDER_UPDATE_FRAME == 0 && size == last_size {
            continue;
        }
        let [w, h] = size;
        if w <= 0 || h <= 0 {
            continue;
        }
        last_size = size;

        let mut stride = (w as usize * 4).next_multiple_of(64);
        let blocks = (stride * h as usize) / 64;
        buf.resize(blocks, Block([0; 64]));
        let mut sw_size = [w as c_int, h as c_int];
        let mut params = [
            RenderParam { ty: MPV_RENDER_PARAM_SW_SIZE, data: sw_size.as_mut_ptr() as *mut c_void },
            RenderParam { ty: MPV_RENDER_PARAM_SW_FORMAT, data: format.as_ptr() as *mut c_void },
            RenderParam { ty: MPV_RENDER_PARAM_SW_STRIDE, data: &mut stride as *mut usize as *mut c_void },
            RenderParam { ty: MPV_RENDER_PARAM_SW_POINTER, data: buf.as_mut_ptr() as *mut c_void },
            RenderParam { ty: MPV_RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        if unsafe { (api.render_render)(ctx.0, params.as_mut_ptr()) } < 0 {
            continue;
        }

        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, blocks * 64) };
        let mut pixels = Vec::with_capacity(w as usize * h as usize);
        for row in bytes.chunks_exact(stride) {
            pixels.extend(row[..w as usize * 4].chunks_exact(4).map(|p| Color32::from_rgb(p[0], p[1], p[2])));
        }
        let image = ColorImage::new([w as usize, h as usize], pixels);
        *shared.frame.lock().unwrap() = Some(image);
        egui_ctx.request_repaint();
    }
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Plays a generated test pattern with a subtitle and checks that frames
    /// come out of the software renderer. Needs libmpv-2.dll in target/<profile>.
    #[test]
    fn renders_frames_and_subtitles() {
        let profile_dir = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![profile_dir];
        paths.extend(std::env::split_paths(&path));
        unsafe { std::env::set_var("PATH", std::env::join_paths(paths).unwrap()) };

        let mpv = Mpv::new(egui::Context::default()).expect("libmpv");
        mpv.set("mute", "yes");
        mpv.set_render_size(320, 180);
        mpv.command(&["loadfile", "av://lavfi:testsrc=duration=5:size=640x360:rate=25"]).unwrap();

        let srt = std::env::temp_dir().join("afterglow-test.srt");
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:05,000\nHello subtitles\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut loaded, mut frames, mut subs_ok) = (false, 0, false);
        while Instant::now() < deadline && frames < 10 {
            for ev in mpv.poll_events() {
                if let Event::FileLoaded = ev {
                    loaded = true;
                    mpv.command(&["sub-add", &srt.to_string_lossy(), "select"]).unwrap();
                    subs_ok = mpv.tracks("sub").iter().any(|t| t.selected);
                }
            }
            if let Some(img) = mpv.take_frame() {
                assert_eq!(img.size, [320, 180]);
                assert!(img.pixels.iter().any(|p| *p != Color32::BLACK), "frame is all black");
                frames += 1;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(loaded, "file never loaded");
        assert!(subs_ok, "subtitle track not selected");
        assert!(frames >= 10, "only got {frames} frames");
        assert!(mpv.get_f64("time-pos").unwrap() > 0.0);
    }
}
