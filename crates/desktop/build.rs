fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icon.ico");
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("../../packaging/icon.ico")
        .set("ProductName", "Flutter Inspector")
        .set("FileDescription", "Flutter Inspector")
        .compile()
        .expect("Failed to embed the Windows application icon");
}
