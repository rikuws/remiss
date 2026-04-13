use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest_path = Path::new(&manifest_dir);
    let bridge_source = manifest_path.join("scripts/code-tour-bridge.mjs");
    let bundle_output = Path::new(&out_dir).join("code-tour-bridge.bundle.mjs");

    println!("cargo:rerun-if-changed=scripts/code-tour-bridge.mjs");
    println!("cargo:rerun-if-changed=package.json");
    println!("cargo:rerun-if-changed=package-lock.json");

    let esbuild_path = manifest_path.join("node_modules/.bin/esbuild");

    if !esbuild_path.exists() {
        run_npm_install(manifest_path);
    }

    let bundled = if esbuild_path.exists() {
        try_bundle(&esbuild_path, &bridge_source, &bundle_output)
    } else {
        false
    };

    if !bundled {
        fs::copy(&bridge_source, &bundle_output)
            .expect("Failed to copy bridge script as fallback");
        println!(
            "cargo:warning=esbuild not available — using unbundled bridge script. \
             The code tour feature will require node_modules at runtime. \
             Run `npm install` to enable bundling."
        );
    }
}

fn run_npm_install(project_dir: &Path) {
    let npm = if Path::new("/opt/homebrew/bin/npm").exists() {
        "/opt/homebrew/bin/npm"
    } else if Path::new("/usr/local/bin/npm").exists() {
        "/usr/local/bin/npm"
    } else {
        "npm"
    };

    match Command::new(npm)
        .arg("install")
        .current_dir(project_dir)
        .output()
    {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("cargo:warning=npm install failed: {stderr}");
        }
        Err(err) => {
            println!("cargo:warning=Could not run npm install: {err}");
        }
    }
}

fn try_bundle(esbuild: &PathBuf, source: &Path, output: &Path) -> bool {
    let result = Command::new(esbuild)
        .args([
            source.to_str().unwrap(),
            "--bundle",
            "--platform=node",
            "--format=esm",
            &format!("--outfile={}", output.display()),
        ])
        .output();

    match result {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            println!("cargo:warning=esbuild failed: {stderr}");
            false
        }
        Err(err) => {
            println!("cargo:warning=Could not run esbuild: {err}");
            false
        }
    }
}
