use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Only run npm build in release mode. Dev builds use the Vite dev server.
    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile != "release" {
        return;
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let frontend = manifest.join("frontend");
    let static_dir = manifest.join("static");

    if !frontend.exists() {
        return;
    }

    println!("cargo:warning=Building frontend (npm run build)...");

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };

    // Install deps if node_modules is missing.
    if !frontend.join("node_modules").exists() {
        let status = Command::new(npm)
            .args(["install"])
            .current_dir(&frontend)
            .status()
            .expect("npm install failed — is Node.js installed?");
        assert!(status.success(), "npm install exited with {status}");
    }

    let status = Command::new(npm)
        .args(["run", "build"])
        .current_dir(&frontend)
        .status()
        .expect("npm run build failed — is Node.js installed?");
    assert!(status.success(), "npm run build exited with {status}");

    println!(
        "cargo:warning=Frontend built → {}",
        static_dir.display()
    );

    // Re-run if any frontend source file changes.
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");
}
