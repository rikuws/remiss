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
        write_unavailable_bridge(&bundle_output).expect("Failed to write bridge fallback");
        println!(
            "cargo:warning=code tour bridge bundling failed — using unavailable bridge fallback. \
             Run `npm install` and rebuild to enable AI code tours."
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
            "--banner:js=import { createRequire as __ghUiCreateRequire } from 'node:module'; const require = __ghUiCreateRequire(import.meta.url);",
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

fn write_unavailable_bridge(output: &Path) -> std::io::Result<()> {
    fs::write(
        output,
        r#"import { readFileSync } from "node:fs";

const request = JSON.parse(readFileSync(0, "utf8"));
const message =
  "Code tour bridge dependencies could not be prepared during build. Run `npm install` and rebuild to enable AI code tours.";

const providers = ["codex", "copilot"].map((provider) => ({
  provider,
  label: provider === "codex" ? "Codex" : "Copilot",
  available: false,
  authenticated: false,
  message,
  detail: message,
  defaultModel: null
}));

if (request.action === "status") {
  process.stdout.write(JSON.stringify({ providers }));
  process.exit(0);
}

process.stderr.write(message);
process.exit(1);
"#,
    )
}
