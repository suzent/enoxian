use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

fn main() {
    // Always stamp the build, before any early return below: `enox --version`
    // must name the commit and channel even for a debug build or one that skips
    // the frontend.
    emit_build_stamp();

    if std::env::var_os("ENOXIAN_SKIP_FRONTEND_BUILD").is_some() {
        println!("cargo:warning=Skipping frontend build (ENOXIAN_SKIP_FRONTEND_BUILD is set)");
        return;
    }

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

    // Install the exact locked dependency graph whenever node_modules is missing
    // or older than the lockfile. Release CI always runs `npm ci` explicitly
    // before Cargo, but keeping this fallback makes local `cargo build --release`
    // and `enox update --dev` convenient and reproducible: a pull that adds a
    // dependency would otherwise fail in `tsc` against a stale node_modules.
    if node_modules_needs_install(&frontend) {
        let status = Command::new(npm)
            .args(["ci"])
            .current_dir(&frontend)
            .status()
            .expect("npm ci failed — is Node.js installed?");
        assert!(status.success(), "npm ci exited with {status}");
    }

    let status = Command::new(npm)
        .args(["run", "build"])
        .current_dir(&frontend)
        .status()
        .expect("npm run build failed — is Node.js installed?");
    assert!(status.success(), "npm run build exited with {status}");

    println!("cargo:warning=Frontend built → {}", static_dir.display());

    // Re-run if any frontend source file changes.
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/package-lock.json");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");
}

/// True when `npm ci` must run before the frontend can build.
///
/// npm rewrites `node_modules/.package-lock.json` on every install, so a
/// lockfile newer than that stamp means the installed tree no longer matches
/// what the sources import.
fn node_modules_needs_install(frontend: &Path) -> bool {
    let modules = frontend.join("node_modules");
    if !modules.exists() {
        return true;
    }
    let Some(installed) = modified_at(&modules.join(".package-lock.json")) else {
        // No stamp: an incomplete or hand-made tree. Reinstall to be sure.
        return true;
    };
    match modified_at(&frontend.join("package-lock.json")) {
        Some(locked) => locked > installed,
        // No lockfile to compare against; trust the existing install.
        None => false,
    }
}

fn modified_at(path: &Path) -> Option<SystemTime> {
    path.metadata().ok()?.modified().ok()
}

/// Stamps the commit and channel that `enox --version` reports.
///
/// The channel cannot be read off the cargo profile: `enox update --dev` builds
/// a local checkout with `--release`, exactly like the published binaries. Only
/// the release workflow knows it is cutting a release, so it says so through
/// `ENOXIAN_RELEASE_BUILD` and every other build is a dev build.
fn emit_build_stamp() {
    println!("cargo:rerun-if-env-changed=ENOXIAN_RELEASE_BUILD");

    let channel = if std::env::var_os("ENOXIAN_RELEASE_BUILD").is_some_and(|v| v != "0") {
        "release"
    } else {
        "dev"
    };
    println!("cargo:rustc-env=ENOX_BUILD_CHANNEL={channel}");
    println!("cargo:rustc-env=ENOX_GIT_COMMIT={}", git_commit());
}

/// Short commit of the tree being built, `-dirty` when it has uncommitted
/// changes. `unknown` when there is no git to ask — a source tarball, or a
/// vendored build — which is information too, so it is never fatal.
///
/// The stamp is re-taken when HEAD or the reflog moves, so it always names the
/// right commit. A tree that only just became dirty keeps the clean stamp until
/// the next commit or checkout; every release build is clean, and a dev build
/// tells you which commit it started from either way.
fn git_commit() -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-C", &manifest])
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };

    let Some(commit) = git(&["rev-parse", "--short=12", "HEAD"]).filter(|c| !c.is_empty()) else {
        return "unknown".to_string();
    };

    // Rebuild the stamp when HEAD moves. `git rev-parse --git-path` resolves
    // worktrees and submodules, where .git is a file rather than a directory.
    for path in ["HEAD", "logs/HEAD"] {
        if let Some(resolved) = git(&["rev-parse", "--git-path", path]) {
            let resolved = PathBuf::from(&manifest).join(resolved);
            if resolved.exists() {
                println!("cargo:rerun-if-changed={}", resolved.display());
            }
        }
    }

    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|out| !out.is_empty());
    if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    }
}
