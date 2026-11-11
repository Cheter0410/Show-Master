//! Gives the Windows executable its icon and version information.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set("ProductName", "Show Master");
        res.set("FileDescription", "Show Master");
        if let Err(e) = res.compile() {
            // Not fatal: the program works without an embedded icon.
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
