// Puts the program icon and version details into marginalia-node-tray.exe on Windows, so Explorer, the Start menu,
// Task Manager and the installer show the mark. Other systems skip it.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../packaging/app.ico");
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("../packaging/app.ico");
            res.set("FileDescription", "Marginalia compute node tray icon");
            res.set("ProductName", "Marginalia compute node");
            res.set("OriginalFilename", "marginalia-node-tray.exe");
            if let Err(e) = res.compile() {
                // A missing resource compiler is no reason to fail the build: the program works without its icon.
                println!("cargo:warning=could not add the icon to the program: {e}");
            }
        }
    }
}
