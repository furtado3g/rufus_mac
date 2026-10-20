fn main() {
    // Build scripts run on the host. Embed the manifest only when the host is
    // Windows (the release pipeline builds Windows artifacts on Windows); the
    // `#[cfg(windows)]` here therefore refers to the host platform.
    //
    // The CLI stays `asInvoker` so `list` and `analyze` work without a UAC
    // prompt; writing to a physical drive fails with access denied unless the
    // user relaunches the terminal elevated.
    #[cfg(windows)]
    {
        let mut resource = winres::WindowsResource::new();
        resource.set_manifest(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="io.image2usb.cli" version="0.1.0.0"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>"#,
        );
        if let Err(err) = resource.compile() {
            println!("cargo:warning=failed to embed Windows manifest: {err}");
        }
    }
}
