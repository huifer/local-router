//! Builds the React console so `include_str!` always has fresh assets.
//!
//! The frontend is optional at build time only when a previous `dist` exists;
//! a clean checkout must build it once (see README / Dockerfile), otherwise
//! the build fails with a clear message instead of a missing-file error.

use std::path::Path;
use std::process::Command;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let frontend = manifest.join("frontend");
    let dist = frontend.join("dist");
    let index = dist.join("index.html");

    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");

    let source_newer = |built: &Path| -> bool {
        let Some(built_time) = mtime(built) else { return true };
        newest_source(&frontend).map(|t| t > built_time).unwrap_or(false)
    };

    if !index.exists() || source_newer(&index) {
        // Doubles as an escape hatch: in a Docker dependency-warmup stage
        // there is no frontend source, so a stub dist keeps cargo happy.
        if std::env::var("LOCAL_ROUTER_SKIP_FRONTEND").is_ok() {
            println!("cargo:warning=LOCAL_ROUTER_SKIP_FRONTEND is set; using a stub console");
            std::fs::create_dir_all(dist.join("assets")).ok();
            std::fs::write(dist.join("index.html"), "<!doctype html><title>stub</title>").ok();
            std::fs::write(dist.join("assets/app.js"), "").ok();
            std::fs::write(dist.join("assets/style.css"), "").ok();
            return;
        }
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        let status = Command::new(npm)
            .arg("run")
            .arg("build")
            .current_dir(&frontend)
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => panic!(
                "the frontend build failed with {status}; run `npm install && npm run build` in ./frontend and retry"
            ),
            Err(err) => panic!(
                "could not run npm to build the console ({err}); build ./frontend once, or ship a prebuilt frontend/dist"
            ),
        }
    }
}

fn mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The newest mtime among the console's sources (one level of nesting is plenty).
fn newest_source(root: &Path) -> Option<std::time::SystemTime> {
    let mut newest = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == "node_modules" || n == "dist") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if let Some(time) = mtime(&path) {
                if newest.map(|seen| time > seen).unwrap_or(true) {
                    newest = Some(time);
                }
            }
        }
    }
    newest
}
