//! Build information for `--version` and the desktop app, and (on Windows) the program's
//! version details and icon, so the .exe says what it is in Explorer and to virus scanners.

fn main() {
    println!("cargo:rerun-if-env-changed=JETSTREAM_BUILD");
    println!("cargo:rerun-if-env-changed=JETSTREAM_COMMIT");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let build = std::env::var("JETSTREAM_BUILD").ok().filter(|b| !b.is_empty());
    let commit = std::env::var("JETSTREAM_COMMIT").ok().filter(|c| !c.is_empty()).map(|c| c.chars().take(7).collect::<String>());
    let text = match (&build, &commit) {
        (Some(b), Some(c)) => format!("{version} (build {b}, {c})"),
        (Some(b), None) => format!("{version} (build {b})"),
        (None, Some(c)) => format!("{version} ({c})"),
        (None, None) => format!("{version} (local build)"),
    };
    println!("cargo:rustc-env=JETSTREAM_VERSION_TEXT={text}");
    println!("cargo:rustc-env=JETSTREAM_BUILD_NUMBER={}", build.unwrap_or_default());
    println!("cargo:rustc-env=JETSTREAM_COMMIT_SHORT={}", commit.unwrap_or_default());

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../src-tauri/icons/icon.ico")
            .set("ProductName", "JetStream")
            .set("FileDescription", "JetStream upload engine")
            .set("CompanyName", "NOAA JetStream")
            .set("LegalCopyright", "NOAA JetStream")
            .set("ProductVersion", &text)
            .set("OriginalFilename", "jetstream-transfer.exe");
        if let Err(err) = res.compile() {
            // Version details are nice to have; never fail the build over them.
            println!("cargo:warning=Windows version resource not embedded: {err}");
        }
    }
}
