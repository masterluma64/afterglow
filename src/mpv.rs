//! Minimal libmpv binding, loaded at runtime from `libmpv-2.dll`.
//!
//! Video is rendered with mpv's OpenGL render API straight into the UI's GL
//! context when one is available. Otherwise mpv's software renderer draws on
//! a dedicated thread and frames are handed to the UI as `egui::ColorImage`s.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use eframe::egui::{self, Color32, ColorImage};
use eframe::glow::{self, HasContext};
use libloading::Library;

pub const DLL_NAME: &str = "libmpv-2.dll";
/// Where Windows builds of libmpv can be downloaded.
pub const DOWNLOAD_URL: &str = "https://github.com/shinchiro/mpv-winbuild-cmake/releases";

const MPV_FORMAT_INT64: c_int = 4;
const MPV_FORMAT_DOUBLE: c_int = 5;

const MPV_EVENT_NONE: c_int = 0;
const MPV_EVENT_END_FILE: c_int = 7;
const MPV_EVENT_FILE_LOADED: c_int = 8;
const MPV_END_FILE_REASON_EOF: c_int = 0;
const MPV_END_FILE_REASON_ERROR: c_int = 4;

const MPV_RENDER_PARAM_INVALID: c_int = 0;
const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const MPV_RENDER_PARAM_OPENGL_FBO: c_int = 3;
const MPV_RENDER_PARAM_FLIP_Y: c_int = 4;
const MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME: c_int = 12;
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

#[repr(C)]
struct OpenGlInitParams {
    get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct OpenGlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

type Handle = *mut c_void;
type UpdateFn = unsafe extern "C" fn(*mut c_void);

/// Looks up OpenGL functions in the UI's GL context.
pub type GetProcAddress = Arc<dyn Fn(&CStr) -> *const c_void + Send + Sync>;

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

/// The libmpv client API major version this binding is written against.
const CLIENT_API_MAJOR: u64 = 2;

impl Api {
    fn load() -> Result<Self, String> {
        let lib = unsafe { Library::new(DLL_NAME) }.map_err(|e| {
            let beside_exe = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(DLL_NAME)));
            match beside_exe {
                Some(path) if path.exists() => format!("{DLL_NAME} is there but could not be loaded: {e}"),
                Some(path) => format!("{DLL_NAME} was not found. Afterglow looks for it at {}.", path.display()),
                None => format!("{DLL_NAME} could not be loaded: {e}"),
            }
        })?;
        let version: libloading::Symbol<unsafe extern "C" fn() -> std::ffi::c_ulong> =
            unsafe { lib.get(b"mpv_client_api_version\0") }.map_err(|_| format!("{DLL_NAME} is not a libmpv library."))?;
        let version = u64::from(unsafe { version() });
        let (major, minor) = (version >> 16, version & 0xffff);
        if major != CLIENT_API_MAJOR {
            return Err(format!(
                "{DLL_NAME} provides libmpv API {major}.{minor}, but Afterglow needs {CLIENT_API_MAJOR}.x."
            ));
        }
        macro_rules! sym {
            ($name:literal) => {
                *unsafe { lib.get($name) }
                    .map_err(|e| format!("{DLL_NAME} (API {major}.{minor}) is too old: {e}"))?
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

#[derive(Clone, Debug)]
pub struct Chapter {
    pub title: String,
    /// Start time in seconds.
    pub time: f64,
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

/// How video reaches the screen.
enum Render {
    Software { ctx: Ptr, shared: Arc<RenderShared>, thread: Option<JoinHandle<()>> },
    OpenGl(Arc<GlRender>),
}

pub struct Mpv {
    api: Arc<Api>,
    handle: Ptr,
    render: Render,
}

impl Mpv {
    /// Starts mpv. With `gl`, video is rendered on the GPU in the UI's OpenGL
    /// context, which must be current on this thread; without it (or if that
    /// fails) it is rendered in software.
    pub fn new(egui_ctx: egui::Context, gl: Option<GetProcAddress>) -> Result<Self, String> {
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
            // Decode on the GPU where it is known to work, falling back to the CPU.
            ("hwdec", "auto-safe"),
        ];
        for (k, v) in options {
            let (k, v) = (cstr(k), cstr(v));
            unsafe { (api.set_option_string)(handle, k.as_ptr(), v.as_ptr()) };
        }
        if let Err(e) = api.check(unsafe { (api.initialize)(handle) }) {
            unsafe { (api.terminate_destroy)(handle) };
            return Err(format!("mpv_initialize failed: {e}"));
        }

        let gl_render = gl.and_then(|gl| match GlRender::new(api.clone(), Ptr(handle), gl, egui_ctx.clone()) {
            Ok(render) => Some(render),
            Err(e) => {
                eprintln!("OpenGL video output unavailable, rendering in software: {e}");
                None
            }
        });
        let render = match gl_render {
            Some(render) => Render::OpenGl(render),
            None => match Self::software_render(&api, handle, egui_ctx) {
                Ok(render) => render,
                Err(e) => {
                    unsafe { (api.terminate_destroy)(handle) };
                    return Err(e);
                }
            },
        };
        Ok(Self { api, handle: Ptr(handle), render })
    }

    fn software_render(api: &Arc<Api>, handle: Handle, egui_ctx: egui::Context) -> Result<Render, String> {
        let sw = cstr("sw");
        let mut params = [
            RenderParam { ty: MPV_RENDER_PARAM_API_TYPE, data: sw.as_ptr() as *mut c_void },
            RenderParam { ty: MPV_RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        let mut render_ctx = std::ptr::null_mut();
        let code = unsafe { (api.render_create)(&mut render_ctx, handle, params.as_mut_ptr()) };
        api.check(code).map_err(|e| format!("mpv_render_context_create failed: {e}"))?;

        let shared = Arc::new(RenderShared::default());
        unsafe {
            (api.render_set_update_callback)(
                render_ctx,
                Some(on_render_update),
                Arc::as_ptr(&shared) as *mut c_void,
            )
        };

        let thread = {
            let api = api.clone();
            let shared = shared.clone();
            let render_ctx = Ptr(render_ctx);
            std::thread::Builder::new()
                .name("mpv-render".into())
                .spawn(move || render_loop(&api, render_ctx, &shared, &egui_ctx))
        };
        match thread {
            Ok(thread) => Ok(Render::Software { ctx: Ptr(render_ctx), shared, thread: Some(thread) }),
            Err(e) => {
                unsafe { (api.render_free)(render_ctx) };
                Err(e.to_string())
            }
        }
    }

    /// Size in physical pixels the video should be rendered at in software.
    pub fn set_render_size(&self, w: u32, h: u32) {
        let Render::Software { shared, .. } = &self.render else { return };
        let size = [w as i32, h as i32];
        if shared.state.lock().unwrap().size != size {
            shared.poke(|st| st.size = size);
        }
    }

    /// Latest frame rendered in software, if a new one is available.
    pub fn take_frame(&self) -> Option<ColorImage> {
        let Render::Software { shared, .. } = &self.render else { return None };
        shared.frame.lock().unwrap().take()
    }

    /// A callback that draws the video, fitted into `rect`, on the GPU. `None`
    /// when rendering in software.
    pub fn paint_callback(&self, rect: egui::Rect) -> Option<egui::PaintCallback> {
        let Render::OpenGl(render) = &self.render else { return None };
        let render = render.clone();
        let callback = eframe::egui_glow::CallbackFn::new(move |info, painter| render.paint(&info, painter.gl()));
        Some(egui::PaintCallback { rect, callback: Arc::new(callback) })
    }

    /// Stops showing the last frame drawn on the GPU until mpv draws a new one,
    /// so a previous video never shows under the next.
    pub fn forget_frame(&self) {
        if let Render::OpenGl(render) = &self.render {
            render.state.lock().unwrap().has_frame = false;
        }
    }

    /// Frees GPU resources. Must be called with the UI's GL context current,
    /// before the context goes away.
    pub fn release_gl(&self, gl: &glow::Context) {
        if let Render::OpenGl(render) = &self.render {
            render.release(gl);
        }
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

    /// The loaded file's chapters, in order.
    pub fn chapters(&self) -> Vec<Chapter> {
        let count = self.get_i64("chapter-list/count").unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let time = self.get_f64(&format!("chapter-list/{i}/time"))?;
                let title = self
                    .get_str(&format!("chapter-list/{i}/title"))
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| format!("Chapter {}", i + 1));
                Some(Chapter { title, time })
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
        match &mut self.render {
            Render::Software { ctx, shared, thread } => {
                shared.poke(|st| st.quit = true);
                if let Some(t) = thread.take() {
                    let _ = t.join();
                }
                unsafe {
                    (self.api.render_set_update_callback)(ctx.0, None, std::ptr::null_mut());
                    (self.api.render_free)(ctx.0);
                }
            }
            Render::OpenGl(render) => {
                if render.state.lock().unwrap().ctx.is_some() {
                    // The render context can only be freed with its GL context
                    // current, and mpv must not be destroyed before it. This
                    // only happens if `release_gl` was skipped, so leave both
                    // to the end of the process.
                    eprintln!("mpv: the GL renderer was not released");
                    return;
                }
            }
        }
        unsafe { (self.api.terminate_destroy)(self.handle.0) };
    }
}

unsafe extern "C" fn get_gl_proc_address(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    let lookup = unsafe { &*(ctx as *const GetProcAddress) };
    lookup(unsafe { CStr::from_ptr(name) }) as *mut c_void
}

unsafe extern "C" fn on_gl_render_update(ctx: *mut c_void) {
    // Called from mpv's threads; must not call back into mpv.
    let render = unsafe { &*(ctx as *const GlRender) };
    render.egui_ctx.request_repaint();
}

/// An offscreen texture that mpv draws into, copied to the screen each paint.
struct GlTarget {
    fbo: glow::Framebuffer,
    texture: glow::Texture,
    size: [i32; 2],
}

struct GlState {
    /// mpv's render context; `None` once released.
    ctx: Option<Ptr>,
    target: Option<GlTarget>,
    /// Whether `target` holds a frame of the current video.
    has_frame: bool,
}

/// Video rendered by mpv in the UI's OpenGL context. Every GL call happens on
/// the UI thread with its context current: on creation, in paint callbacks
/// and on release.
struct GlRender {
    api: Arc<Api>,
    state: Mutex<GlState>,
    egui_ctx: egui::Context,
    /// mpv holds a pointer to this.
    _get_proc_address: Box<GetProcAddress>,
}

impl GlRender {
    fn new(api: Arc<Api>, handle: Ptr, get_proc_address: GetProcAddress, egui_ctx: egui::Context) -> Result<Arc<Self>, String> {
        let get_proc_address = Box::new(get_proc_address);
        let opengl = cstr("opengl");
        let mut init = OpenGlInitParams {
            get_proc_address: Some(get_gl_proc_address),
            get_proc_address_ctx: &*get_proc_address as *const GetProcAddress as *mut c_void,
        };
        let mut params = [
            RenderParam { ty: MPV_RENDER_PARAM_API_TYPE, data: opengl.as_ptr() as *mut c_void },
            RenderParam { ty: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, data: &mut init as *mut _ as *mut c_void },
            RenderParam { ty: MPV_RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        let mut ctx = std::ptr::null_mut();
        api.check(unsafe { (api.render_create)(&mut ctx, handle.0, params.as_mut_ptr()) })?;
        let render = Arc::new(Self {
            api,
            state: Mutex::new(GlState { ctx: Some(Ptr(ctx)), target: None, has_frame: false }),
            egui_ctx,
            _get_proc_address: get_proc_address,
        });
        unsafe {
            (render.api.render_set_update_callback)(ctx, Some(on_gl_render_update), Arc::as_ptr(&render) as *mut c_void)
        };
        Ok(render)
    }

    /// Renders a new frame if mpv has one (or the size changed), then copies
    /// the latest frame into the callback's viewport.
    fn paint(&self, info: &egui::PaintCallbackInfo, gl: &glow::Context) {
        let mut st = self.state.lock().unwrap();
        let Some(ctx) = st.ctx else { return };
        let vp = info.viewport_in_pixels();
        let size = [vp.width_px, vp.height_px];
        if size[0] <= 0 || size[1] <= 0 {
            return;
        }
        unsafe {
            // egui may be drawing into its own framebuffer rather than the window's.
            let draw_fb = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32;
            let draw_fb = std::num::NonZeroU32::new(draw_fb).map(glow::NativeFramebuffer);

            let resized = st.target.as_ref().map(|t| t.size) != Some(size);
            if resized {
                if let Some(old) = st.target.take() {
                    gl.delete_framebuffer(old.fbo);
                    gl.delete_texture(old.texture);
                }
                match create_target(gl, size) {
                    Ok(target) => st.target = Some(target),
                    Err(e) => {
                        eprintln!("could not create the video framebuffer: {e}");
                        gl.bind_framebuffer(glow::FRAMEBUFFER, draw_fb);
                        return;
                    }
                }
            }
            let fbo = st.target.as_ref().unwrap().fbo;

            let flags = (self.api.render_update)(ctx.0);
            let new_frame = flags & MPV_RENDER_UPDATE_FRAME != 0;
            if resized || new_frame {
                let mut target = OpenGlFbo { fbo: fbo.0.get() as c_int, w: size[0], h: size[1], internal_format: glow::RGBA8 as c_int };
                // The framebuffer's origin is bottom-left, like the screen's.
                let mut flip_y: c_int = 1;
                // Never wait for the frame's display time: this is the UI thread.
                let mut block: c_int = 0;
                let mut params = [
                    RenderParam { ty: MPV_RENDER_PARAM_OPENGL_FBO, data: &mut target as *mut _ as *mut c_void },
                    RenderParam { ty: MPV_RENDER_PARAM_FLIP_Y, data: &mut flip_y as *mut _ as *mut c_void },
                    RenderParam { ty: MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME, data: &mut block as *mut _ as *mut c_void },
                    RenderParam { ty: MPV_RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
                ];
                if (self.api.render_render)(ctx.0, params.as_mut_ptr()) >= 0 && new_frame {
                    st.has_frame = true;
                }
                // Undo state mpv may leave behind that egui doesn't reset itself.
                gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.bind_vertex_array(None);
            }

            if st.has_frame {
                let [w, h] = size;
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, draw_fb);
                gl.blit_framebuffer(
                    0, 0, w, h,
                    vp.left_px, vp.from_bottom_px, vp.left_px + w, vp.from_bottom_px + h,
                    glow::COLOR_BUFFER_BIT, glow::NEAREST,
                );
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, draw_fb);
        }
    }

    fn release(&self, gl: &glow::Context) {
        let mut st = self.state.lock().unwrap();
        if let Some(ctx) = st.ctx.take() {
            unsafe {
                (self.api.render_set_update_callback)(ctx.0, None, std::ptr::null_mut());
                (self.api.render_free)(ctx.0);
            }
        }
        if let Some(target) = st.target.take() {
            unsafe {
                gl.delete_framebuffer(target.fbo);
                gl.delete_texture(target.texture);
            }
        }
    }
}

unsafe fn create_target(gl: &glow::Context, [w, h]: [i32; 2]) -> Result<GlTarget, String> {
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0,
            glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fbo);
            gl.delete_texture(texture);
            return Err(format!("framebuffer incomplete ({status:#x})"));
        }
        Ok(GlTarget { fbo, texture, size: [w, h] })
    }
}

/// A headless mpv instance that only opens files to read their length.
/// It stays paused with null outputs, so nothing is played. Use it from one thread.
pub struct Prober {
    api: Arc<Api>,
    handle: Ptr,
}

impl Prober {
    pub fn new() -> Result<Self, String> {
        let api = Arc::new(Api::load()?);
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("mpv_create failed".into());
        }
        let options = [
            ("vo", "null"),
            ("ao", "null"),
            ("sid", "no"),
            ("sub-auto", "no"),
            ("pause", "yes"),
            ("idle", "yes"),
            ("ytdl", "no"),
            ("input-default-bindings", "no"),
        ];
        for (k, v) in options {
            let (k, v) = (cstr(k), cstr(v));
            unsafe { (api.set_option_string)(handle, k.as_ptr(), v.as_ptr()) };
        }
        if let Err(e) = api.check(unsafe { (api.initialize)(handle) }) {
            unsafe { (api.terminate_destroy)(handle) };
            return Err(format!("mpv_initialize failed: {e}"));
        }
        Ok(Self { api, handle: Ptr(handle) })
    }

    /// Length of a video in seconds, or `None` if it can't be opened in time.
    pub fn duration(&self, path: &Path, timeout: std::time::Duration) -> Option<f64> {
        let (cmd, file, mode) = (cstr("loadfile"), cstr(&path.to_string_lossy()), cstr("replace"));
        let mut args = [cmd.as_ptr(), file.as_ptr(), mode.as_ptr(), std::ptr::null()];
        self.api.check(unsafe { (self.api.command)(self.handle.0, args.as_mut_ptr()) }).ok()?;

        let deadline = std::time::Instant::now() + timeout;
        let mut duration = None;
        let mut ended = false;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            let ev = unsafe { &*(self.api.wait_event)(self.handle.0, left.as_secs_f64()) };
            match ev.event_id {
                MPV_EVENT_FILE_LOADED => {
                    let name = cstr("duration");
                    let mut v = 0f64;
                    let code = unsafe {
                        (self.api.get_property)(self.handle.0, name.as_ptr(), MPV_FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void)
                    };
                    duration = (code >= 0 && v > 0.0).then_some(v);
                    break;
                }
                MPV_EVENT_END_FILE => {
                    ended = true;
                    break;
                }
                _ => {}
            }
        }
        if ended {
            return None;
        }
        // Unload the file and wait for it to finish, so its events don't leak
        // into the next probe.
        let stop = cstr("stop");
        let mut args = [stop.as_ptr(), std::ptr::null()];
        unsafe { (self.api.command)(self.handle.0, args.as_mut_ptr()) };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            let ev = unsafe { &*(self.api.wait_event)(self.handle.0, left.as_secs_f64()) };
            if matches!(ev.event_id, MPV_EVENT_END_FILE | MPV_EVENT_NONE) {
                break;
            }
        }
        duration
    }
}

impl Drop for Prober {
    fn drop(&mut self) {
        unsafe { (self.api.terminate_destroy)(self.handle.0) };
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

    /// Lets tests find libmpv-2.dll, which the build script puts in target/<profile>.
    pub(super) fn use_built_dll() {
        let profile_dir = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![profile_dir];
        paths.extend(std::env::split_paths(&path));
        unsafe { std::env::set_var("PATH", std::env::join_paths(paths).unwrap()) };
    }

    /// A silent 8 kHz mono WAV file in the temp folder. Its header states the exact length.
    pub(super) fn silent_wav(name: &str, seconds: u32) -> std::path::PathBuf {
        let data_len = 8000 * 2 * seconds;
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data_len).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        f.extend_from_slice(&1u16.to_le_bytes()); // PCM
        f.extend_from_slice(&1u16.to_le_bytes()); // mono
        f.extend_from_slice(&8000u32.to_le_bytes()); // sample rate
        f.extend_from_slice(&16000u32.to_le_bytes()); // bytes per second
        f.extend_from_slice(&2u16.to_le_bytes()); // block align
        f.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        f.extend_from_slice(b"data");
        f.extend_from_slice(&data_len.to_le_bytes());
        f.resize(f.len() + data_len as usize, 0);
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, f).unwrap();
        path
    }

    /// Plays a generated test pattern with a subtitle and checks that frames
    /// come out of the software renderer. Needs libmpv-2.dll in target/<profile>.
    #[test]
    fn renders_frames_and_subtitles() {
        use_built_dll();

        let mpv = Mpv::new(egui::Context::default(), None).expect("libmpv");
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
            // Setting the render size draws an empty black frame before the
            // file is loaded, so only frames with picture in them count.
            if let Some(img) = mpv.take_frame() {
                assert_eq!(img.size, [320, 180]);
                if img.pixels.iter().any(|p| *p != Color32::BLACK) {
                    frames += 1;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(loaded, "file never loaded");
        assert!(subs_ok, "subtitle track not selected");
        assert!(frames >= 10, "only got {frames} frames");
        assert!(mpv.get_f64("time-pos").unwrap() > 0.0);
    }
}

#[cfg(test)]
mod chapter_tests {
    use super::tests::{silent_wav, use_built_dll};
    use super::*;
    use std::time::{Duration, Instant};

    /// mpv turns each segment of an EDL playlist into a chapter.
    #[test]
    fn reads_chapters() {
        use_built_dll();
        let wav = silent_wav("afterglow-chapters.wav", 6);
        let source = wav.to_string_lossy().into_owned();
        let segment = |start: u32, title: &str| {
            format!("%{}%{source},start={start},length=2,title={title}\n", source.len())
        };
        let edl = std::env::temp_dir().join("afterglow-chapters.edl");
        let text = format!("# mpv EDL v0\n{}{}{}", segment(0, "Intro"), segment(2, "Middle"), segment(4, "End"));
        std::fs::write(&edl, text).unwrap();

        let mpv = Mpv::new(egui::Context::default(), None).expect("libmpv");
        mpv.set("pause", "yes");
        mpv.load_file(&edl, 0.0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !mpv.poll_events().iter().any(|e| matches!(e, Event::FileLoaded)) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let chapters = mpv.chapters();
        let titles: Vec<_> = chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Intro", "Middle", "End"]);
        let times: Vec<_> = chapters.iter().map(|c| c.time.round()).collect();
        assert_eq!(times, [0.0, 2.0, 4.0]);
        std::fs::remove_file(edl).unwrap();
        std::fs::remove_file(wav).unwrap();
    }
}

#[cfg(test)]
mod prober_tests {
    use super::*;
    use super::tests::{silent_wav, use_built_dll};

    #[test]
    fn reads_durations_and_rejects_bad_files() {
        use_built_dll();

        let (seven, three) = (silent_wav("afterglow-probe-7.wav", 7), silent_wav("afterglow-probe-3.wav", 3));

        let prober = Prober::new().expect("libmpv");
        let timeout = std::time::Duration::from_secs(5);
        let d = prober.duration(&seven, timeout).expect("duration");
        assert!((d - 7.0).abs() < 0.05, "got {d}");
        // Probing again works: the previous file's events don't interfere.
        let d = prober.duration(&three, timeout).expect("duration");
        assert!((d - 3.0).abs() < 0.05, "got {d}");
        std::fs::remove_file(seven).unwrap();
        std::fs::remove_file(three).unwrap();

        let bogus = std::env::temp_dir().join("afterglow-not-a-video.mkv");
        std::fs::write(&bogus, b"definitely not a video").unwrap();
        assert_eq!(prober.duration(&bogus, timeout), None);
        std::fs::remove_file(bogus).unwrap();
    }
}
