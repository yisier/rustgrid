//! Windows resource embedding: the app icon plus a `VERSIONINFO` block so the produced
//! `RustGrid.exe` shows the product name, description and version in Explorer's Properties
//! dialog instead of empty strings.

fn main() {
    #[cfg(target_os = "windows")]
    {
        use std::path::PathBuf;

        println!("cargo:rerun-if-changed=assets/logo.rc");
        println!("cargo:rerun-if-changed=assets/logo.ico");
        println!("cargo:rerun-if-changed=build.rs");

        let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let logo_rc = std::fs::read_to_string(manifest.join("assets/logo.rc"))
            .expect("failed to read assets/logo.rc");
        let icon = manifest
            .join("assets/logo.ico")
            .to_string_lossy()
            .replace('\\', "/");
        // windres resolves paths inside a `.rc` relative to that `.rc`'s directory, but the
        // generated script lives in OUT_DIR — so rewrite the icon reference to an absolute path.
        let logo_rc = logo_rc.replace("assets/logo.ico", &icon);

        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
        let mut parts: Vec<u32> = version
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        parts.resize(4, 0);
        let (major, minor, patch, build) = (parts[0], parts[1], parts[2], parts[3]);

        let script = format!(
            r#"{logo_rc}
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},{build}
PRODUCTVERSION {major},{minor},{patch},{build}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "yisier"
      VALUE "FileDescription", "RustGrid"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "RustGrid"
      VALUE "OriginalFilename", "RustGrid.exe"
      VALUE "ProductName", "RustGrid"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
        );

        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("RustGrid.rc");
        std::fs::write(&out, script).expect("failed to write the generated resource script");
        let _ = embed_resource::compile(&out, embed_resource::NONE).manifest_optional();
    }
}
