//! Keeps DLLs from being loaded from the current folder or PATH.
//!
//! By default Windows also looks in the current folder (which can be the
//! folder of a video that was opened) and in every PATH folder, so a DLL
//! planted there could be loaded in place of a missing libmpv-2.dll, or of
//! an optional one libmpv looks for, such as a GPU decoder's.

/// The executable's folder, System32, and folders added with `AddDllDirectory`.
const LOAD_LIBRARY_SEARCH_DEFAULT_DIRS: u32 = 0x0000_1000;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetDefaultDllDirectories(flags: u32) -> i32;
}

/// Call first thing in `main`, before any DLL is loaded by name.
pub fn restrict() {
    // Returns 0 on failure; DLLs are then searched for as before.
    if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) } == 0 {
        eprintln!("could not restrict where DLLs are loaded from: {}", std::io::Error::last_os_error());
    }
}
