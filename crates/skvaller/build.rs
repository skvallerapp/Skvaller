//! On macOS, embeds Info.plist in the executable, so the menu bar names the app
//! Skvaller rather than after its file, `skvaller`.

fn main() {
    println!("cargo::rerun-if-changed=Info.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let plist = concat!(env!("CARGO_MANIFEST_DIR"), "/Info.plist");
        println!("cargo::rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{plist}");
    }
}
