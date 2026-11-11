//! Gives the Windows executable its icon and version information. Works for
//! native Windows builds and for cross-builds from Linux (needs MinGW's windres).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon.ico");
    res.set("ProductName", "Show Master");
    res.set("FileDescription", "Show Master");
    if let Err(e) = res.compile() {
        // Not fatal: the program works without an embedded icon.
        println!("cargo:warning=could not embed the Windows icon: {e}");
    }
}
