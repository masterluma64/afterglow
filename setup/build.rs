//! Builds the release afterglow.exe into the setup, with the app's version,
//! icon and a manifest. Build the app first (`cargo build --release`), or
//! point `AFTERGLOW_EXE` at another afterglow.exe.

use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let version = app_version(&root.join("Cargo.toml"));
    println!("cargo:rustc-env=AFTERGLOW_VERSION={version}");

    println!("cargo:rerun-if-env-changed=AFTERGLOW_EXE");
    let app = std::env::var_os("AFTERGLOW_EXE").map(PathBuf::from).unwrap_or_else(|| root.join("target/release/afterglow.exe"));
    for path in [&app, &root.join("Cargo.toml"), &root.join("LICENSE"), &root.join("THIRD-PARTY-NOTICES.txt"), &root.join("assets/icon.ico")] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let bytes = std::fs::read(&app).unwrap_or_else(|e| panic!("{} can't be read ({e}); build the app first with `cargo build --release`", app.display()));
    // Its version info holds the version as UTF-16 text.
    let wide: Vec<u8> = version.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    if !bytes.windows(wide.len()).any(|w| w == wide) {
        panic!("{} isn't version {version}; build it again with `cargo build --release`", app.display());
    }
    println!("cargo:rustc-env=AFTERGLOW_PAYLOAD={}", app.display());

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let manifest = out.join("setup.manifest");
    std::fs::write(&manifest, MANIFEST).unwrap();
    let rc = out.join("setup.rc");
    std::fs::write(&rc, resources(&version, &root.join("assets/icon.ico"), &manifest)).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE).manifest_required().unwrap();
}

/// The `version` of the app's package.
fn app_version(cargo_toml: &Path) -> String {
    let text = std::fs::read_to_string(cargo_toml).unwrap();
    let package = text.split("[package]").nth(1).unwrap();
    let line = package.lines().find(|l| l.trim_start().starts_with("version")).unwrap();
    line.split('"').nth(1).unwrap().to_string()
}

fn resources(version: &str, icon: &Path, manifest: &Path) -> String {
    let mut parts = version.split(['.', '-']).map(|p| p.parse::<u16>().unwrap_or(0));
    let numbers: Vec<String> = (0..4).map(|_| parts.next().unwrap_or(0).to_string()).collect();
    let numbers = numbers.join(",");
    let path = |p: &Path| p.display().to_string().replace('\\', "/");
    let (icon, manifest) = (path(icon), path(manifest));
    format!(
        r#"1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Afterglow setup"
      VALUE "ProductName", "Afterglow"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "afterglow-setup.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}

/// Common Controls 6, for the folder picker; `asInvoker`, or Windows would ask
/// for administrator rights for an exe with "setup" in its name; and sharp
/// text on scaled displays.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="afterglow.setup" version="1.0.0.0" processorArchitecture="amd64"/>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;
