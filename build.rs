extern crate winres;

fn main() {
    if cfg!(target_os = "windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icon.ico");
        // 设置 manifest 以启用 Common Controls v6 (Visual Styles)
        // 这通常能解决 Native Windows GUI 的很多兼容性问题和 Entry Point 错误
        res.set_manifest(r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<dependency>
    <dependentAssembly>
        <assemblyIdentity
            type="win32"
            name="Microsoft.Windows.Common-Controls"
            version="6.0.0.0"
            processorArchitecture="*"
            publicKeyToken="6595b64144ccf1df"
            language="*"
        />
    </dependentAssembly>
</dependency>
</assembly>
"#);
        if let Err(e) = res.compile() {
            eprintln!("Error compiling windows resources: {}", e);
        }
    }
}
