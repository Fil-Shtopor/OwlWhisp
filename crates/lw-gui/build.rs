use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons/icon.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon = root.join("../../assets/icons/icon.ico").canonicalize().unwrap();
    let icon = icon.to_string_lossy().replace('\\', "\\\\");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numbers: Vec<_> = version
        .split(['.', '-'])
        .take(3)
        .map(|part| part.parse::<u16>().expect("numeric application version"))
        .collect();
    let numeric = format!("{},{},{},0", numbers[0], numbers[1], numbers[2]);
    let resource = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("owlwhisp.rc");
    fs::write(
        &resource,
        format!(
            r#"#include <winver.h>
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK VS_FFI_FILEFLAGSMASK
FILEFLAGS 0
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Fil-Shtopor\0"
      VALUE "FileDescription", "OwlWhisp\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "OwlWhisp\0"
      VALUE "OriginalFilename", "OwlWhisp.exe\0"
      VALUE "ProductName", "OwlWhisp\0"
      VALUE "ProductVersion", "{version}\0"
      VALUE "LegalCopyright", "OwlWhisp contributors\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#
        ),
    )
    .unwrap();
    embed_resource::compile_for(&resource, ["owlwhisp"], embed_resource::NONE)
        .manifest_required()
        .expect("Windows icon and version resources must be embedded");
}
