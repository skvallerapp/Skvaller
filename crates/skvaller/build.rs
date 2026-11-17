//! On macOS, embeds Info.plist in the executable, so the menu bar shows the app as
//! Skvaller rather than by its file name, `skvaller`.

fn main() {
    println!("cargo::rerun-if-changed=Info.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let plist = concat!(env!("CARGO_MANIFEST_DIR"), "/Info.plist");
        // One argument each: `-Wl,` would split a path with a comma.
        for arg in ["-sectcreate", "__TEXT", "__info_plist", plist] {
            println!("cargo::rustc-link-arg-bins=-Xlinker");
            println!("cargo::rustc-link-arg-bins={arg}");
        }
    }
}
