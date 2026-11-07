//! Embeds the app icon in the Windows executable, for Explorer and the taskbar.

use std::{env, fs, path::Path, process::Command};

const ICON: &str = "assets/icons/amplo.ico";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON}");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // A missing tool costs only the icon, so it must not stop the build.
    if let Err(err) = embed_icon() {
        println!("cargo:warning=amplo.exe has no icon: {err}");
    }
}

fn embed_icon() -> Result<(), String> {
    let out_dir = env::var("OUT_DIR").map_err(|err| err.to_string())?;
    let out_dir = Path::new(&out_dir);
    let icon = Path::new(ICON).canonicalize().map_err(|err| err.to_string())?;
    let script = out_dir.join("amplo.rc");
    // The resource compilers read a backslash as an escape.
    let line = format!("1 ICON \"{}\"\n", icon.display().to_string().replace("\\\\?\\", "").replace('\\', "/"));
    fs::write(&script, line).map_err(|err| err.to_string())?;

    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let (object, mut command) = if msvc {
        let object = out_dir.join("amplo.res");
        let mut command = Command::new("rc");
        command.arg("/nologo").arg("/fo").arg(&object).arg(&script);
        (object, command)
    } else {
        let object = out_dir.join("amplo-icon.o");
        let mut command = Command::new("windres");
        command.arg(&script).args(["-O", "coff", "-o"]).arg(&object);
        (object, command)
    };
    let output = command.output().map_err(|err| format!("{command:?} did not start: {err}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    println!("cargo:rustc-link-arg-bins={}", object.display());
    Ok(())
}
