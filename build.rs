extern crate winres;

fn main() {
    if cfg!(target_os = "windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icon.ico");
        // debug 默认 asInvoker，便于 `cargo run`；release 或 `--features admin` 请求管理员
        let level = if cfg!(feature = "admin") || cfg!(not(debug_assertions)) {
            "requireAdministrator"
        } else {
            "asInvoker"
        };
        let manifest = format!(
            r#"
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
<trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
        <requestedPrivileges>
            <requestedExecutionLevel level="{level}" uiAccess="false" />
        </requestedPrivileges>
    </security>
</trustInfo>
</assembly>
"#
        );
        res.set_manifest(&manifest);
        if let Err(e) = res.compile() {
            eprintln!("Error compiling windows resources: {}", e);
        }
    }
}
