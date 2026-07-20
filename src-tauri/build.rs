use std::fs;

use tauri_build::{Attributes, WindowsAttributes};

fn main() {
    println!("cargo:rerun-if-changed=../target/release/hook.dll");

    // The Tauri bundle reads this resource from src-tauri. Copy it for both
    // debug and release builds so an MSI can never silently package a stale
    // hook from an earlier test run.
    fs::copy("../target/release/hook.dll", "hook.dll")
        .expect("Could not copy the freshly built release hook.dll into the Tauri resources.");

    if cfg!(debug_assertions) {
        tauri_build::build();
    } else {
        let windows = WindowsAttributes::new().app_manifest(include_str!("manifest.xml"));

        tauri_build::try_build(Attributes::new().windows_attributes(windows))
            .expect("Could not build Tauri app.")
    }
}
