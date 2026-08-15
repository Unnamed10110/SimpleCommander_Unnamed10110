fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("ProductName", "SimpleCommander");
    res.set("FileDescription", "SimpleCommander — dual-pane file explorer");
    res.set("InternalName", "simplecommander");
    res.set("OriginalFilename", "simplecommander.exe");
    res.compile().expect("failed to embed Windows icon resource");
}
