//! Embeds the Windows resources: the manifest, the icon and the version block.
//!
//! The manifest is the point. Without `requireAdministrator` in it the user has
//! to pick "Run as administrator" by hand every single time, because the
//! ACPI-WMI classes refuse reads to an unelevated process — and the refusal
//! arrives late enough to look like a bug rather than a permission problem.
//!
//! It goes into release builds only, which is not squeamishness: the resource is
//! linked into every target of the package, the test harness included, and a
//! test binary that demands elevation cannot be launched by cargo at all (os
//! error 740). So `cargo test` and `cargo build` produce ordinary programs —
//! started from an elevated shell during development — and the exe people
//! download asks for rights by itself.
//!
//! `cargo test --release` is the one casualty, and needs an elevated shell.
//!
//! The manifest is embedded as a resource rather than through the linker's
//! `/MANIFESTINPUT`, because rustc already supplies a default manifest of its own
//! and mt.exe refuses to merge two that disagree about `level`.

use std::io;

/// `requireAdministrator` so Windows asks once, at launch, instead of letting the
/// program start and fail on its first read. The `supportedOS` ids are Vista
/// through Windows 10/11, which is what tells Windows not to lie to us about
/// versions; `dpiAware` keeps the menu and the toast from being bitmap-stretched.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{e2011457-1546-43c5-a5fe-008deee3d3f0}"/>
      <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}"/>
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">permonitorv2,permonitor</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/msi-hotkeys.ico");

    // Cross-compiling to anything else would not produce a working program, but
    // it should not fail in the resource compiler either.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }

    let mut resources = winresource::WindowsResource::new();
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        resources.set_manifest(MANIFEST);
    }
    resources.set_icon("assets/msi-hotkeys.ico");
    resources.set("ProductName", "msi-hotkeys");
    resources.set("FileDescription", env!("CARGO_PKG_DESCRIPTION"));
    resources.set("LegalCopyright", "MIT licensed. No warranty.");
    resources.compile()
}
