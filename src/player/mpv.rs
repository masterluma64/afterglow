//! Minimal libmpv binding, loaded at runtime from `libmpv-2.dll`.
//!
//! Video is rendered with mpv's OpenGL render API straight into the UI's GL
//! context. Thumbnails (and tests) use mpv's software renderer instead, which
//! draws on a dedicated thread and hands frames over as `egui::ColorImage`s.

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

/// Evens out loud and quiet parts ("night mode"): quiet dialogue gets louder
/// and loud scenes quieter. Set as mpv's `af` (audio filters).
pub const VOLUME_LEVELER: &str = "@level:dynaudnorm=f=75:g=25:p=0.55";

const MPV_FORMAT_STRING: c_int = 1;
const MPV_FORMAT_FLAG: c_int = 3;
const MPV_FORMAT_INT64: c_int = 4;
const MPV_FORMAT_DOUBLE: c_int = 5;
const MPV_FORMAT_NODE: c_int = 6;
const MPV_FORMAT_NODE_ARRAY: c_int = 7;
const MPV_FORMAT_NODE_MAP: c_int = 8;
const MPV_FORMAT_BYTE_ARRAY: c_int = 9;

const MPV_EVENT_NONE: c_int = 0;
const MPV_EVENT_END_FILE: c_int = 7;
const MPV_EVENT_FILE_LOADED: c_int = 8;
const MPV_EVENT_PLAYBACK_RESTART: c_int = 21;
const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;
const MPV_EVENT_COMMAND_REPLY: c_int = 5;
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

/// For instances that open files nobody asked to play (lengths, previews):
/// don't follow what a file refers to (playlists, EDL, linked segments,
/// ordered chapters), which could be a network share or a web address.
const NO_REFERENCES: (&str, &str) = ("access-references", "no");

/// How soon to repaint for a new video frame. egui turns an immediate request
/// into two UI passes (to let layout settle); any delay makes it one, which
/// halves the UI work during playback.
const NEW_FRAME_REPAINT: std::time::Duration = std::time::Duration::from_micros(1);

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
    playlist_entry_id: i64,
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
    command_async: unsafe extern "C" fn(Handle, u64, *mut *const c_char) -> c_int,
    observe_property: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    set_property_async: unsafe extern "C" fn(Handle, u64, *const c_char, c_int, *mut c_void) -> c_int,
    get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    // Only tests read properties as strings.
    #[cfg_attr(not(test), allow(dead_code))]
    get_property_string: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    #[cfg_attr(not(test), allow(dead_code))]
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
        // A libmpv-2.dll next to the exe comes first, then the one packed
        // into the exe (release builds).
        let lib = match unsafe { Library::new(DLL_NAME) } {
            Ok(lib) => lib,
            Err(e) => match super::packed::unpacked() {
                Some(Ok(path)) => unsafe { Library::new(&path) }
                    .map_err(|e| format!("libmpv was unpacked to {} but could not be loaded: {e}", path.display()))?,
                Some(Err(e)) => return Err(e),
                None => {
                    let beside_exe = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(DLL_NAME)));
                    return Err(match beside_exe {
                        Some(path) if path.exists() => format!("{DLL_NAME} is there but could not be loaded: {e}"),
                        Some(path) => format!("{DLL_NAME} was not found. Afterglow looks for it at {}.", path.display()),
                        None => format!("{DLL_NAME} could not be loaded: {e}"),
                    });
                }
            },
        };
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
            command_async: sym!(b"mpv_command_async\0"),
            observe_property: sym!(b"mpv_observe_property\0"),
            set_property_async: sym!(b"mpv_set_property_async\0"),            get_property: sym!(b"mpv_get_property\0"),
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
    /// `entry` is the file's playlist entry id (see [`Mpv::load_file`]).
    EndFile { eof: bool, error: bool, entry: i64 },
    /// Playback started after loading a file or seeking: the current frame
    /// is from where playback now is.
    PlaybackRestart,
    /// A command sent with [`Mpv::send_tagged`] finished, with what it
    /// returned (`Node::None` for most commands).
    CommandDone { tag: u64, ok: bool, result: Node },
    /// An observed property changed (see [`Mpv::observe`]). `Node::None`
    /// means it is currently unavailable.
    Property { name: String, value: Node },
}

/// A property value, converted from mpv's `mpv_node`.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    None,
    Str(String),
    Flag(bool),
    Int(i64),
    Double(f64),
    Array(Vec<Node>),
    Map(Vec<(String, Node)>),
    Bytes(Vec<u8>),
}

impl Node {
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Node::Double(d) => Some(d),
            Node::Int(i) => Some(i as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Node::Flag(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Node::Int(i) => Some(i),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value of `key` in a map.
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn items(&self) -> &[Node] {
        match self {
            Node::Array(items) => items,
            _ => &[],
        }
    }
}

/// The picture from a `screenshot-raw` command asked for in `rgba`.
pub fn screenshot_image(result: &Node) -> Option<ColorImage> {
    let size = |key| result.get(key).and_then(Node::as_i64).and_then(|n| usize::try_from(n).ok());
    let (w, h, stride) = (size("w")?, size("h")?, size("stride")?);
    let Some(Node::Bytes(data)) = result.get("data") else { return None };
    if result.get("format").and_then(Node::as_str) != Some("rgba") || w == 0 || stride < w * 4 || data.len() < stride * h {
        return None;
    }
    let pixels = data.chunks(stride).take(h).flat_map(|row| row[..w * 4].as_chunks::<4>().0);
    Some(ColorImage::new([w, h], pixels.map(|p| Color32::from_rgb(p[0], p[1], p[2])).collect()))
}

#[repr(C)]
union RawNodeValue {
    string: *const c_char,
    flag: c_int,
    int64: i64,
    double: f64,
    list: *const RawNodeList,
    bytes: *const RawByteArray,
}

#[repr(C)]
struct RawNode {
    u: RawNodeValue,
    format: c_int,
}

#[repr(C)]
struct RawNodeList {
    num: c_int,
    values: *const RawNode,
    keys: *const *const c_char,
}

#[repr(C)]
struct RawByteArray {
    data: *const u8,
    size: usize,
}

#[repr(C)]
struct RawEventProperty {
    name: *const c_char,
    format: c_int,
    data: *const c_void,
}

/// Copies an `mpv_node` (which mpv frees itself) into a [`Node`].
unsafe fn convert_node(raw: &RawNode) -> Node {
    let text = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe {
        match raw.format {
            MPV_FORMAT_STRING => Node::Str(text(raw.u.string)),
            MPV_FORMAT_FLAG => Node::Flag(raw.u.flag != 0),
            MPV_FORMAT_INT64 => Node::Int(raw.u.int64),
            MPV_FORMAT_DOUBLE => Node::Double(raw.u.double),
            MPV_FORMAT_BYTE_ARRAY if !raw.u.bytes.is_null() => {
                let bytes = &*raw.u.bytes;
                Node::Bytes(if bytes.data.is_null() { Vec::new() } else { std::slice::from_raw_parts(bytes.data, bytes.size).to_vec() })
            }
            MPV_FORMAT_NODE_ARRAY | MPV_FORMAT_NODE_MAP if !raw.u.list.is_null() => {
                let list = &*raw.u.list;
                let values = (0..list.num.max(0) as usize).map(|i| convert_node(&*list.values.add(i)));
                if raw.format == MPV_FORMAT_NODE_ARRAY {
                    Node::Array(values.collect())
                } else {
                    let keys = (0..list.num.max(0) as usize).map(|i| text(*list.keys.add(i)));
                    Node::Map(keys.zip(values).collect())
                }
            }
            _ => Node::None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: i64,
    /// "video", "audio" or "sub".
    pub kind: String,
    pub label: String,
    pub selected: bool,
}

impl Track {
    /// The tracks in mpv's `track-list` property.
    pub fn list(node: &Node) -> Vec<Track> {
        node.items()
            .iter()
            .filter_map(|t| {
                let id = t.get("id")?.as_f64()? as i64;
                let kind = t.get("type")?.as_str()?.to_string();
                let text = |key: &str| t.get(key).and_then(Node::as_str).filter(|s| !s.is_empty());
                let flag = |key: &str| t.get(key).and_then(Node::as_bool).unwrap_or(false);
                let mut label = match (text("title"), text("lang")) {
                    (Some(title), Some(lang)) => format!("{title} [{lang}]"),
                    (Some(text), None) | (None, Some(text)) => text.to_string(),
                    (None, None) => format!("Track {id}"),
                };
                if flag("external") {
                    label.push_str(" (external)");
                }
                Some(Track { id, kind, label, selected: flag("selected") })
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct Chapter {
    pub title: String,
    /// Start time in seconds.
    pub time: f64,
}

impl Chapter {
    /// The chapters in mpv's `chapter-list` property, in order.
    pub fn list(node: &Node) -> Vec<Chapter> {
        node.items()
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let time = c.get("time")?.as_f64()?;
                let title = c
                    .get("title")
                    .and_then(Node::as_str)
                    .filter(|t| !t.trim().is_empty())
                    .map_or_else(|| format!("Chapter {}", i + 1), str::to_string);
                Some(Chapter { title, time })
            })
            .collect()
    }
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
    /// Render the current frame again even if mpv has no new one.
    redraw: bool,
    size: [i32; 2],
}

#[derive(Default)]
struct RenderShared {
    state: Mutex<RenderState>,
    cv: Condvar,
    frame: Mutex<Option<ColorImage>>,
    /// While set, rendered frames are thrown away (see `Mpv::hide_frames`).
    hidden: std::sync::atomic::AtomicBool,
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
    /// Starts mpv, rendering video on the GPU in the UI's OpenGL context,
    /// which must be current on this thread.
    pub fn new(egui_ctx: egui::Context, gl: GetProcAddress) -> Result<Self, String> {
        Self::create(egui_ctx, Some(gl), &[])
    }

    /// Starts mpv rendering in software, for tests that need no window.
    #[cfg(test)]
    pub fn software(egui_ctx: egui::Context) -> Result<Self, String> {
        Self::create(egui_ctx, None, &[])
    }

    /// A silent, paused instance rendering small frames in software, for
    /// previews: load a file and seek, then take the frames.
    pub fn thumbnailer(egui_ctx: egui::Context) -> Result<Self, String> {
        let options = [
            ("pause", "yes"),
            ("ao", "null"),
            ("aid", "no"),
            ("sid", "no"),
            ("sub-auto", "no"),
            ("hwdec", "no"),
            // Speed over quality: previews are small and short-lived.
            ("hr-seek", "no"),
            ("vd-lavc-fast", "yes"),
            ("vd-lavc-skiploopfilter", "all"),
            ("demuxer-readahead-secs", "0"),
            ("cache", "no"),
            NO_REFERENCES,
        ];
        Self::create(egui_ctx, None, &options)
    }

    fn create(egui_ctx: egui::Context, gl: Option<GetProcAddress>, extra: &[(&str, &str)]) -> Result<Self, String> {
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
            // libmpv's defaults already, stated so they can't change under us:
            // no mpv.conf and no user scripts.
            ("config", "no"),
            ("load-scripts", "no"),
            // Decode on the GPU where it is known to work, falling back to the CPU.
            ("hwdec", "auto-safe"),
        ];
        for (k, v) in options.iter().chain(extra) {
            let (k, v) = (cstr(k), cstr(v));
            unsafe { (api.set_option_string)(handle, k.as_ptr(), v.as_ptr()) };
        }
        if let Err(e) = api.check(unsafe { (api.initialize)(handle) }) {
            unsafe { (api.terminate_destroy)(handle) };
            return Err(format!("mpv_initialize failed: {e}"));
        }

        let render = match gl {
            Some(gl) => GlRender::new(api.clone(), Ptr(handle), gl, egui_ctx).map(Render::OpenGl),
            None => Self::software_render(&api, handle, egui_ctx),
        };
        let render = match render {
            Ok(render) => render,
            Err(e) => {
                unsafe { (api.terminate_destroy)(handle) };
                return Err(e);
            }
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
        let frame = shared.frame.lock().unwrap().take();
        frame.filter(|_| !shared.hidden.load(std::sync::atomic::Ordering::Acquire))
    }

    /// A callback that draws the video, fitted into `rect`, on the GPU. `None`
    /// when rendering in software.
    pub fn paint_callback(&self, rect: egui::Rect) -> Option<egui::PaintCallback> {
        let Render::OpenGl(render) = &self.render else { return None };
        let render = render.clone();
        let callback = eframe::egui_glow::CallbackFn::new(move |info, painter| render.paint(&info, painter.gl()));
        Some(egui::PaintCallback { rect, callback: Arc::new(callback) })
    }

    /// Shows no video (black) until [`Self::show_frames`]. While a new file
    /// starts, mpv still draws the previous one's last frame now and then;
    /// this keeps it from flashing up before the new video.
    pub fn hide_frames(&self) {
        match &self.render {
            Render::OpenGl(render) => {
                let mut st = render.state.lock().unwrap();
                st.hidden = true;
                st.has_frame = false;
            }
            Render::Software { shared, .. } => {
                shared.hidden.store(true, std::sync::atomic::Ordering::Release);
                shared.frame.lock().unwrap().take();
            }
        }
    }

    /// Shows video again, starting with the current frame. Call it once
    /// playback of the new file has started (`Event::PlaybackRestart`).
    pub fn show_frames(&self) {
        match &self.render {
            Render::OpenGl(render) => {
                let mut st = render.state.lock().unwrap();
                st.hidden = false;
                st.redraw = true;
                render.egui_ctx.request_repaint();
            }
            Render::Software { shared, .. } => {
                shared.hidden.store(false, std::sync::atomic::Ordering::Release);
                shared.poke(|st| st.redraw = true);
            }
        }
    }

    /// Frees GPU resources. Must be called with the UI's GL context current,
    /// before the context goes away.
    pub fn release_gl(&self, gl: &glow::Context) {
        if let Render::OpenGl(render) = &self.render {
            render.release(gl);
        }
    }
    /// Runs a command and waits for it. Can block while mpv is busy (e.g.
    /// seeking), so on the UI thread prefer [`Self::send`] where the result
    /// isn't needed.
    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        self.api.check(unsafe { (self.api.command)(self.handle.0, ptrs.as_mut_ptr()) })
    }

    /// Queues a command without waiting for it to run. Commands and property
    /// changes run in the order they were sent.
    pub fn send(&self, args: &[&str]) {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe { (self.api.command_async)(self.handle.0, 0, ptrs.as_mut_ptr()) };
    }

    /// Like [`Self::send`], reporting the outcome as [`Event::CommandDone`]
    /// with `tag` (which must not be 0).
    pub fn send_tagged(&self, tag: u64, args: &[&str]) {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe { (self.api.command_async)(self.handle.0, tag, ptrs.as_mut_ptr()) };
    }

    /// Sets a property without waiting for it to be applied.
    pub fn set(&self, name: &str, value: &str) {
        let (n, v) = (cstr(name), cstr(value));
        let mut value = v.as_ptr();
        unsafe {
            (self.api.set_property_async)(
                self.handle.0,
                0,
                n.as_ptr(),
                MPV_FORMAT_STRING,
                &mut value as *mut *const c_char as *mut c_void,
            )
        };
    }

    /// Asks for [`Event::Property`] whenever `name` changes, starting with
    /// its current value. Reading properties directly can block the caller
    /// while mpv is busy; observing never does.
    pub fn observe(&self, name: &str) {
        let n = cstr(name);
        unsafe { (self.api.observe_property)(self.handle.0, 0, n.as_ptr(), MPV_FORMAT_NODE) };
    }

    #[cfg(test)]
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

    #[cfg(test)]
    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let n = cstr(name);
        let mut v = 0f64;
        let code = unsafe {
            (self.api.get_property)(self.handle.0, n.as_ptr(), MPV_FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void)
        };
        (code >= 0).then_some(v)
    }

    #[cfg(test)]
    pub fn get_flag(&self, name: &str) -> bool {
        self.get_str(name).as_deref() == Some("yes")
    }

    /// Opens `path` at `start` seconds. With a `tag` other than 0, the reply
    /// is an [`Event::CommandDone`] whose result holds the file's
    /// `playlist_entry_id`, which its [`Event::EndFile`] carries too.
    pub fn load_file(&self, path: &Path, start: f64, tag: u64) {
        // `start` is a global option, so it has to be reset for every file.
        self.set("start", &if start > 0.0 { format!("{start:.3}") } else { "none".into() });
        self.send_tagged(tag, &["loadfile", &path.to_string_lossy(), "replace"]);
    }

    pub fn poll_events(&self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            let ev = unsafe { &*(self.api.wait_event)(self.handle.0, 0.0) };
            match ev.event_id {
                MPV_EVENT_NONE => break,
                MPV_EVENT_FILE_LOADED => out.push(Event::FileLoaded),
                MPV_EVENT_PLAYBACK_RESTART => out.push(Event::PlaybackRestart),
                MPV_EVENT_END_FILE if !ev.data.is_null() => {
                    let end = unsafe { &*(ev.data as *const RawEndFile) };
                    out.push(Event::EndFile {
                        eof: end.reason == MPV_END_FILE_REASON_EOF,
                        error: end.reason == MPV_END_FILE_REASON_ERROR,
                        entry: end.playlist_entry_id,
                    });
                }
                MPV_EVENT_COMMAND_REPLY if ev.reply_userdata != 0 => {
                    // `data` is an `mpv_event_command`, which holds just the result node.
                    let result = if ev.data.is_null() { Node::None } else { unsafe { convert_node(&*(ev.data as *const RawNode)) } };
                    out.push(Event::CommandDone { tag: ev.reply_userdata, ok: ev.error >= 0, result });
                }
                MPV_EVENT_PROPERTY_CHANGE if !ev.data.is_null() => {
                    let prop = unsafe { &*(ev.data as *const RawEventProperty) };
                    let name = unsafe { CStr::from_ptr(prop.name) }.to_string_lossy().into_owned();
                    let value = match prop.format {
                        MPV_FORMAT_NODE if !prop.data.is_null() => unsafe { convert_node(&*(prop.data as *const RawNode)) },
                        _ => Node::None,
                    };
                    out.push(Event::Property { name, value });
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
    render.egui_ctx.request_repaint_after(NEW_FRAME_REPAINT);
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
    /// See [`Mpv::hide_frames`].
    hidden: bool,
    /// Render the current frame again even if mpv has no new one.
    redraw: bool,
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
            state: Mutex::new(GlState { ctx: Some(Ptr(ctx)), target: None, has_frame: false, hidden: false, redraw: false }),
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
            let redraw = std::mem::take(&mut st.redraw);
            if resized || new_frame || redraw {
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
                if (self.api.render_render)(ctx.0, params.as_mut_ptr()) >= 0 && !st.hidden {
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

/// What [`Prober::probe`] reads from a video.
#[derive(Debug, PartialEq)]
pub struct Info {
    /// Length in seconds, if mpv knows it.
    pub duration: Option<f64>,
    /// The title stored in the file, if it has one.
    pub title: Option<String>,
}

/// A headless mpv instance that only opens files to read their length and title.
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
            ("config", "no"),
            ("load-scripts", "no"),
            NO_REFERENCES,
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

    /// A video's length and title, or `None` if it can't be opened in time.
    pub fn probe(&self, path: &Path, timeout: std::time::Duration) -> Option<Info> {
        let (cmd, file, mode) = (cstr("loadfile"), cstr(&path.to_string_lossy()), cstr("replace"));
        let mut args = [cmd.as_ptr(), file.as_ptr(), mode.as_ptr(), std::ptr::null()];
        self.api.check(unsafe { (self.api.command)(self.handle.0, args.as_mut_ptr()) }).ok()?;

        let deadline = std::time::Instant::now() + timeout;
        let mut info = None;
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
                    let duration = (code >= 0 && v > 0.0).then_some(v);
                    info = Some(Info { duration, title: self.get_str("metadata/by-key/title") });
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
        info
    }

    fn get_str(&self, name: &str) -> Option<String> {
        let n = cstr(name);
        let p = unsafe { (self.api.get_property_string)(self.handle.0, n.as_ptr()) };
        if p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { (self.api.free)(p as *mut c_void) };
        Some(s)
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
            (st.size, std::mem::take(&mut st.redraw))
        };
        let (size, redraw) = size;

        let flags = unsafe { (api.render_update)(ctx.0) };
        if flags & MPV_RENDER_UPDATE_FRAME == 0 && size == last_size && !redraw {
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
            pixels.extend(row[..w as usize * 4].as_chunks::<4>().0.iter().map(|p| Color32::from_rgb(p[0], p[1], p[2])));
        }
        let image = ColorImage::new([w as usize, h as usize], pixels);
        if shared.hidden.load(std::sync::atomic::Ordering::Acquire) {
            continue;
        }
        *shared.frame.lock().unwrap() = Some(image);
        egui_ctx.request_repaint_after(NEW_FRAME_REPAINT);
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

    /// Encodes a test pattern into a seekable video file in the temp folder,
    /// with a keyframe every second.
    pub(super) fn encode_test_video(name: &str, seconds: u32) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        let api = Api::load().unwrap();
        let handle = unsafe { (api.create)() };
        let out = path.to_string_lossy().into_owned();
        let options = [("o", out.as_str()), ("of", "matroska"), ("ovc", "mpeg4"), ("ovcopts", "g=25,qscale=5"), ("ao", "null")];
        for (k, v) in options {
            let (k, v) = (cstr(k), cstr(v));
            unsafe { (api.set_option_string)(handle, k.as_ptr(), v.as_ptr()) };
        }
        api.check(unsafe { (api.initialize)(handle) }).unwrap();
        let source = format!("av://lavfi:testsrc=duration={seconds}:size=640x360:rate=25");
        let (cmd, file) = (cstr("loadfile"), cstr(&source));
        let mut args = [cmd.as_ptr(), file.as_ptr(), std::ptr::null()];
        api.check(unsafe { (api.command)(handle, args.as_mut_ptr()) }).unwrap();
        loop {
            let ev = unsafe { &*(api.wait_event)(handle, 30.0) };
            if matches!(ev.event_id, MPV_EVENT_END_FILE | MPV_EVENT_NONE) {
                break;
            }
        }
        unsafe { (api.terminate_destroy)(handle) };
        assert!(path.metadata().is_ok_and(|m| m.len() > 0), "encoding failed");
        path
    }

    /// Plays a generated test pattern with a subtitle and checks that frames
    /// come out of the software renderer. Needs libmpv-2.dll in target/<profile>.
    #[test]
    fn renders_frames_and_subtitles() {
        use_built_dll();

        let mpv = Mpv::software(egui::Context::default()).expect("libmpv");
        mpv.set("mute", "yes");
        mpv.set_render_size(320, 180);
        mpv.observe("track-list");
        mpv.command(&["loadfile", "av://lavfi:testsrc=duration=5:size=640x360:rate=25"]).unwrap();

        let srt = std::env::temp_dir().join("afterglow-test.srt");
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:05,000\nHello subtitles\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut loaded, mut frames, mut subs_ok) = (false, 0, false);
        while Instant::now() < deadline && frames < 10 {
            for ev in mpv.poll_events() {
                match ev {
                    Event::FileLoaded => {
                        loaded = true;
                        mpv.command(&["sub-add", &srt.to_string_lossy(), "select"]).unwrap();
                    }
                    // The observed track list reports the added subtitle as selected.
                    Event::Property { name, value } if name == "track-list" => {
                        let tracks = Track::list(&value);
                        subs_ok |= tracks.iter().any(|t| t.kind == "sub" && t.selected && t.label.contains("external"));
                    }
                    _ => {}
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
mod thumbnailer_tests {
    use super::tests::{encode_test_video, use_built_dll};
    use super::*;
    use std::time::{Duration, Instant};

    /// The previewer stays paused but still shows a frame after loading and
    /// after each seek.
    #[test]
    fn renders_frames_while_paused() {
        use_built_dll();
        // Generated sources can't seek, so encode a real file first.
        let video = encode_test_video("afterglow-thumbnails.mkv", 10);
        let mpv = Mpv::thumbnailer(egui::Context::default()).expect("libmpv");
        mpv.set_render_size(224, 126);
        mpv.load_file(&video, 2.0, 0);

        let picture = |mpv: &Mpv| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                mpv.poll_events();
                if let Some(img) = mpv.take_frame()
                    && img.pixels.iter().any(|p| *p != Color32::BLACK)
                {
                    return Some(img);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        };
        let first = picture(&mpv).expect("a frame after loading");
        assert_eq!(first.size, [224, 126]);
        assert!(mpv.get_flag("pause"));
        mpv.command(&["seek", "6", "absolute+keyframes"]).unwrap();
        let second = picture(&mpv).expect("a frame after seeking");
        assert_ne!(first.pixels, second.pixels, "the seek shows a different frame");
        assert!((mpv.get_f64("time-pos").unwrap() - 6.0).abs() < 1.0);
        drop(mpv);
        std::fs::remove_file(video).unwrap();
    }

}

#[cfg(test)]
mod subtitle_sync_tests {
    use super::tests::{encode_test_video, use_built_dll};
    use super::*;
    use std::time::{Duration, Instant};

    /// `sub-step` shifts the delay so the next or previous line starts now,
    /// which the sync panel relies on.
    #[test]
    fn steps_to_subtitle_lines() {
        use_built_dll();
        let video = encode_test_video("afterglow-sync.mkv", 10);
        let srt = std::env::temp_dir().join("afterglow-sync.srt");
        let lines = "1\n00:00:03,000 --> 00:00:04,000\nOne\n\n2\n00:00:06,000 --> 00:00:07,000\nTwo\n";
        std::fs::write(&srt, lines).unwrap();

        let mpv = Mpv::software(egui::Context::default()).expect("libmpv");
        mpv.set("pause", "yes");
        mpv.load_file(&video, 5.0, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !mpv.poll_events().iter().any(|e| matches!(e, Event::FileLoaded)) {
            std::thread::sleep(Duration::from_millis(10));
        }
        // Commands sent without waiting report back, with their tag.
        let reply = |tag: u64| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                for ev in mpv.poll_events() {
                    if let Event::CommandDone { tag: t, ok, .. } = ev
                        && t == tag
                    {
                        return Some(ok);
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        };
        mpv.send_tagged(7, &["sub-add", "afterglow-missing.srt"]);
        assert_eq!(reply(7), Some(false), "failures are reported");
        mpv.send_tagged(8, &["sub-add", &srt.to_string_lossy(), "select"]);
        assert_eq!(reply(8), Some(true));
        std::thread::sleep(Duration::from_millis(300));
        let position = mpv.get_f64("time-pos").unwrap();

        // At ~5 s, line two (6 s) is next: showing it now means 1 s earlier.
        mpv.command(&["sub-step", "1"]).unwrap();
        let delay = mpv.get_f64("sub-delay").unwrap();
        assert!((delay - (position - 6.0)).abs() < 0.1, "delay {delay} at {position}");
        mpv.command(&["set", "sub-delay", "0"]).unwrap();
        // Line one (3 s) is the previous one: showing it now means 2 s later.
        mpv.command(&["sub-step", "-1"]).unwrap();
        let delay = mpv.get_f64("sub-delay").unwrap();
        assert!((delay - (position - 3.0)).abs() < 0.1, "delay {delay} at {position}");

        mpv.command(&["set", "sub-speed", &(25.0 / 23.976).to_string()]).unwrap();
        assert!((mpv.get_f64("sub-speed").unwrap() - 25.0 / 23.976).abs() < 1e-6);
        drop(mpv);
        std::fs::remove_file(video).unwrap();
        std::fs::remove_file(srt).unwrap();
    }
}

#[cfg(test)]
mod audio_filter_tests {
    use super::tests::{silent_wav, use_built_dll};
    use super::*;
    use std::time::{Duration, Instant};

    /// The volume leveler is a filter this libmpv build has, and audio keeps
    /// playing through it.
    #[test]
    fn levels_volume() {
        use_built_dll();
        let wav = silent_wav("afterglow-level.wav", 3);
        let mpv = Mpv::software(egui::Context::default()).expect("libmpv");
        mpv.command(&["set", "ao", "null"]).unwrap();
        mpv.command(&["set", "af", VOLUME_LEVELER]).expect("the filter is known");
        mpv.command(&["loadfile", &wav.to_string_lossy()]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while mpv.get_f64("time-pos").unwrap_or(0.0) < 0.5 {
            assert!(Instant::now() < deadline, "playback didn't advance");
            mpv.poll_events();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(mpv.get_str("af").is_some_and(|af| af.contains("dynaudnorm")));
        assert!(mpv.command(&["set", "af", "@level:nosuchfilter"]).is_err(), "unknown filters are refused");
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

        let mpv = Mpv::software(egui::Context::default()).expect("libmpv");
        mpv.set("pause", "yes");
        mpv.observe("chapter-list");
        mpv.load_file(&edl, 0.0, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut chapters = Vec::new();
        while Instant::now() < deadline && chapters.is_empty() {
            for ev in mpv.poll_events() {
                if let Event::Property { name, value } = ev {
                    assert_eq!(name, "chapter-list");
                    chapters = Chapter::list(&value);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let titles: Vec<_> = chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Intro", "Middle", "End"]);
        let times: Vec<_> = chapters.iter().map(|c| c.time.round()).collect();
        assert_eq!(times, [0.0, 2.0, 4.0]);
        std::fs::remove_file(edl).unwrap();
        std::fs::remove_file(wav).unwrap();
    }
}

#[cfg(test)]
mod entry_tests {
    use super::tests::{silent_wav, use_built_dll};
    use super::*;
    use std::time::{Duration, Instant};

    /// The reply to opening a file and the file's end carry the same entry
    /// id, so the end of a replaced file can be told from the new one's.
    #[test]
    fn ends_name_the_file_they_are_about() {
        use_built_dll();
        let (one, two) = (silent_wav("afterglow-entry-1.wav", 1), silent_wav("afterglow-entry-2.wav", 1));
        let mpv = Mpv::software(egui::Context::default()).expect("libmpv");
        mpv.set("mute", "yes");
        let mut events = Vec::new();
        let until = |done: &dyn Fn(&[Event]) -> bool, events: &mut Vec<Event>| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done(events) && Instant::now() < deadline {
                events.extend(mpv.poll_events());
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let replies = |events: &[Event]| -> Vec<i64> {
            let entry = |result: &Node| result.get("playlist_entry_id").and_then(Node::as_i64);
            events.iter().filter_map(|e| match e { Event::CommandDone { tag: 9, result, .. } => entry(result), _ => None }).collect()
        };
        let ends = |events: &[Event]| -> Vec<(i64, bool)> {
            events.iter().filter_map(|e| match e { Event::EndFile { entry, eof, .. } => Some((*entry, *eof)), _ => None }).collect()
        };

        mpv.load_file(&one, 0.0, 9);
        until(&|e| replies(e).len() == 1, &mut events);
        // Replace it before it ends, then let the second play to its end.
        mpv.load_file(&two, 0.0, 9);
        until(&|e| ends(e).iter().any(|&(_, eof)| eof), &mut events);
        let ids = replies(&events);
        assert_eq!(ids.len(), 2, "both opens are answered with an entry id");
        assert_ne!(ids[0], ids[1]);
        assert_eq!(ends(&events), [(ids[0], false), (ids[1], true)], "each end names its own file");
        drop(mpv);
        std::fs::remove_file(one).unwrap();
        std::fs::remove_file(two).unwrap();
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
        let d = prober.probe(&seven, timeout).and_then(|i| i.duration).expect("duration");
        assert!((d - 7.0).abs() < 0.05, "got {d}");
        // Probing again works: the previous file's events don't interfere.
        let info = prober.probe(&three, timeout).expect("info");
        let d = info.duration.expect("duration");
        assert!((d - 3.0).abs() < 0.05, "got {d}");
        assert_eq!(info.title, None, "no title stored");
        std::fs::remove_file(seven).unwrap();
        std::fs::remove_file(three).unwrap();

        let bogus = std::env::temp_dir().join("afterglow-not-a-video.mkv");
        std::fs::write(&bogus, b"definitely not a video").unwrap();
        assert_eq!(prober.probe(&bogus, timeout), None);
        std::fs::remove_file(bogus).unwrap();

        // A WAV file stores its title in an INFO list, as "INAM".
        let titled = silent_wav("afterglow-probe-titled.wav", 2);
        let mut bytes = std::fs::read(&titled).unwrap();
        let name = b"Big Buck Bunny\0\0";
        let mut list = b"INFOINAM".to_vec();
        list.extend_from_slice(&(name.len() as u32).to_le_bytes());
        list.extend_from_slice(name);
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&(list.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&list);
        let riff_len = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_len.to_le_bytes());
        std::fs::write(&titled, bytes).unwrap();
        let info = prober.probe(&titled, timeout).expect("info");
        std::fs::remove_file(titled).unwrap();
        assert_eq!(info.title.as_deref(), Some("Big Buck Bunny"));
    }

    /// A file that refers to other files (here an EDL disguised as .mkv)
    /// isn't followed by the instances that open files without being asked:
    /// it could point at a network share or a web address.
    #[test]
    fn background_instances_do_not_follow_references() {
        use_built_dll();
        let wav = silent_wav("afterglow-reference-target.wav", 4);
        let source = wav.to_string_lossy().into_owned();
        let edl = std::env::temp_dir().join("afterglow-reference.mkv");
        std::fs::write(&edl, format!("# mpv EDL v0\n%{}%{source},0,4\n", source.len())).unwrap();
        let timeout = std::time::Duration::from_secs(5);

        assert_eq!(Prober::new().expect("libmpv").probe(&edl, timeout), None, "the prober doesn't follow it");

        let previews = Mpv::thumbnailer(egui::Context::default()).expect("libmpv");
        previews.load_file(&edl, 0.0, 0);
        let deadline = std::time::Instant::now() + timeout;
        let mut failed = false;
        while std::time::Instant::now() < deadline && !failed {
            failed = previews.poll_events().iter().any(|e| matches!(e, Event::EndFile { error: true, .. }));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(failed, "the previewer doesn't follow it");
        drop(previews);
        std::fs::remove_file(edl).unwrap();
        std::fs::remove_file(wav).unwrap();
    }
}

#[cfg(test)]
mod screenshot_tests {
    use super::tests::use_built_dll;
    use super::*;
    use std::time::{Duration, Instant};

    /// Screenshots work while video is drawn through the render API, as
    /// Afterglow does: saved as a PNG, and as pixels for the clipboard.
    #[test]
    fn takes_screenshots() {
        use_built_dll();
        let mpv = Mpv::software(egui::Context::default()).expect("libmpv");
        mpv.set("mute", "yes");
        mpv.set_render_size(320, 180);
        mpv.command(&["loadfile", "av://lavfi:testsrc=duration=5:size=640x360:rate=25"]).unwrap();
        let path = std::env::temp_dir().join(format!("afterglow-screenshot-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut frames = 0;
        while frames < 5 {
            assert!(Instant::now() < deadline, "no frames");
            mpv.poll_events();
            if mpv.take_frame().is_some_and(|img| img.pixels.iter().any(|p| *p != Color32::BLACK)) {
                frames += 1;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        mpv.command(&["screenshot-to-file", &path.to_string_lossy(), "subtitles"]).expect("screenshot");
        let png = std::fs::read(&path).expect("screenshot written");
        assert!(png.starts_with(b"\x89PNG"), "saved as PNG");
        let _ = std::fs::remove_file(&path);

        mpv.send_tagged(7, &["screenshot-raw", "subtitles", "rgba"]);
        let deadline = Instant::now() + Duration::from_secs(10);
        let image = loop {
            assert!(Instant::now() < deadline, "no raw screenshot");
            let done = mpv.poll_events().into_iter().find_map(|ev| match ev {
                Event::CommandDone { tag: 7, ok, result } => Some((ok, result)),
                _ => None,
            });
            if let Some((ok, result)) = done {
                assert!(ok, "screenshot-raw failed");
                break screenshot_image(&result).expect("an rgba picture");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // The video's own size, not the window's.
        assert_eq!(image.size, [640, 360]);
        assert!(image.pixels.iter().any(|p| *p != Color32::BLACK), "not blank");
    }
}
