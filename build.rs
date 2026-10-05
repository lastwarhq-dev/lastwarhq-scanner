//! Embeds the exe's Windows resources: the version information shown under the file's
//! Properties → Details, taken from Cargo.toml, and the application manifest in `res/`.

use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=res/app.manifest");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let var = |key: &str| env::var(key).unwrap_or_else(|_| panic!("{key} is not set"));
    let (major, minor, patch) = (
        var("CARGO_PKG_VERSION_MAJOR"),
        var("CARGO_PKG_VERSION_MINOR"),
        var("CARGO_PKG_VERSION_PATCH"),
    );
    let manifest = Path::new(&var("CARGO_MANIFEST_DIR")).join("res/app.manifest");
    let strings = [
        ("CompanyName", "lastwarhq-dev".to_string()),
        ("FileDescription", "LastWarHQ Scanner".to_string()),
        ("FileVersion", var("CARGO_PKG_VERSION")),
        ("InternalName", "lastwarhq-scanner".to_string()),
        (
            "LegalCopyright",
            "Copyright (c) 2026 lastwarhq-dev, MIT License".to_string(),
        ),
        ("OriginalFilename", "lastwarhq-scanner.exe".to_string()),
        ("ProductName", "LastWarHQ Scanner".to_string()),
        ("ProductVersion", var("CARGO_PKG_VERSION")),
        ("Comments", var("CARGO_PKG_DESCRIPTION")),
    ]
    .map(|(key, value)| format!("            VALUE \"{key}\", {}\n", rc_string(&value)))
    .concat();
    // 0x40004 is VOS_NT_WINDOWS32 and 1 is VFT_APP, written out so no SDK header is needed.
    // 0x0409 / 1200 is US English text in Unicode.
    let rc = format!(
        "1 24 {manifest}

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3F
FILEFLAGS 0
FILEOS 0x40004
FILETYPE 1
FILESUBTYPE 0
BEGIN
    BLOCK \"StringFileInfo\"
    BEGIN
        BLOCK \"040904B0\"
        BEGIN
{strings}        END
    END
    BLOCK \"VarFileInfo\"
    BEGIN
        VALUE \"Translation\", 0x0409, 1200
    END
END
",
        manifest = rc_string(&manifest.to_string_lossy()),
    );
    let path = Path::new(&var("OUT_DIR")).join("app.rc");
    std::fs::write(&path, rc).expect("cannot write app.rc");
    embed_resource::compile(&path, embed_resource::NONE)
        .manifest_required()
        .expect("cannot embed the exe's resources");
}

/// A resource-script string: quoted, with `"` doubled and `\` escaped.
fn rc_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\"\""))
}
