//! Stamps the running version into the binary at compile time. The release
//! workflow writes APP_VERSION_FILE (the git tag) into the checked-out
//! source tree before calling `cross build` — since cross copies the whole
//! project into its build container as part of compiling it, this avoids
//! depending on cross's environment-variable passthrough (which turned out
//! not to reliably propagate through to rustc in practice). Local dev builds
//! just get "dev" since the file never exists outside CI.
fn main() {
    let version = std::fs::read_to_string("APP_VERSION_FILE")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "dev".to_string());
    println!("cargo:rustc-env=APP_VERSION={version}");
    println!("cargo:rerun-if-changed=APP_VERSION_FILE");
}
