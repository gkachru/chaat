//! Embeds the icon and version information Windows shows for chaat.exe.

fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/chaat.ico");
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("packaging/windows/chaat.ico")
            .set("ProductName", "Chaat")
            .set("FileDescription", "Chaat")
            .set(
                "LegalCopyright",
                "Based on Wren, © 2026 Andrei Antonescu and Wren contributors. MIT licence.",
            );
        if let Err(error) = resource.compile() {
            println!("cargo:warning=could not embed the Windows icon: {error}");
        }
    }
}
