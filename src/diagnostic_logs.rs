use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

use crate::{app_storage, branding::APP_NAME};

const LOG_DIRS: &[&str] = &["copilot-diagnostics", "ai-stack-logs", "checkout-logs"];

pub fn export_diagnostic_logs_zip() -> Result<PathBuf, String> {
    let storage_root = app_storage::data_dir_root();
    let export_dir = dirs::download_dir().unwrap_or_else(|| storage_root.clone());
    fs::create_dir_all(&export_dir).map_err(|error| {
        format!(
            "Failed to create export directory '{}': {error}",
            export_dir.display()
        )
    })?;

    let export_path = unique_export_path(&export_dir);
    let file = File::create(&export_path)
        .map_err(|error| format!("Failed to create '{}': {error}", export_path.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    write_manifest(&mut zip, options, &storage_root)?;

    let mut included_files = 0usize;
    for dir_name in LOG_DIRS {
        let dir = storage_root.join(dir_name);
        if dir.is_dir() {
            add_directory_to_zip(&mut zip, &dir, dir_name, options, &mut included_files)?;
        }
    }

    zip.finish()
        .map_err(|error| format!("Failed to finish '{}': {error}", export_path.display()))?;

    if included_files == 0 {
        let _ = fs::remove_file(&export_path);
        return Err("No Remiss diagnostic log files were found to export.".to_string());
    }

    Ok(export_path)
}

pub fn reveal_export(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg("-R").arg(path).spawn();
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
    }
}

fn write_manifest<W: Write + io::Seek>(
    zip: &mut ZipWriter<W>,
    options: SimpleFileOptions,
    storage_root: &Path,
) -> Result<(), String> {
    let manifest = format!(
        "{APP_NAME} diagnostic log export\nstorageRoot={}\ncreatedAtMs={}\n",
        storage_root.display(),
        now_ms()
    );
    zip.start_file("remiss-logs/manifest.txt", options)
        .map_err(|error| format!("Failed to add manifest to log export: {error}"))?;
    zip.write_all(manifest.as_bytes())
        .map_err(|error| format!("Failed to write log export manifest: {error}"))
}

fn add_directory_to_zip<W: Write + io::Seek>(
    zip: &mut ZipWriter<W>,
    dir: &Path,
    dir_name: &str,
    options: SimpleFileOptions,
    included_files: &mut usize,
) -> Result<(), String> {
    for entry in fs::read_dir(dir)
        .map_err(|error| format!("Failed to read log directory '{}': {error}", dir.display()))?
    {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to read an entry from log directory '{}': {error}",
                dir.display()
            )
        })?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            format!("Failed to inspect log entry '{}': {error}", path.display())
        })?;

        if metadata.is_dir() {
            add_directory_to_zip(zip, &path, dir_name, options, included_files)?;
        } else if metadata.is_file() {
            add_file_to_zip(zip, &path, dir, dir_name, options)?;
            *included_files += 1;
        }
    }
    Ok(())
}

fn add_file_to_zip<W: Write + io::Seek>(
    zip: &mut ZipWriter<W>,
    path: &Path,
    root: &Path,
    root_name: &str,
    options: SimpleFileOptions,
) -> Result<(), String> {
    let archive_name = archive_name_for(path, root, root_name)?;
    zip.start_file(&archive_name, options)
        .map_err(|error| format!("Failed to add '{archive_name}' to log export: {error}"))?;
    let mut file = File::open(path)
        .map_err(|error| format!("Failed to open '{}': {error}", path.display()))?;
    io::copy(&mut file, zip).map_err(|error| {
        format!(
            "Failed to write '{}' to log export: {error}",
            path.display()
        )
    })?;
    Ok(())
}

fn archive_name_for(path: &Path, root: &Path, root_name: &str) -> Result<String, String> {
    let relative = path.strip_prefix(root).map_err(|error| {
        format!(
            "Failed to build archive path for '{}': {error}",
            path.display()
        )
    })?;
    let mut parts = vec!["remiss-logs".to_string(), root_name.to_string()];
    parts.extend(
        relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part.to_string_lossy().to_string()),
                _ => None,
            }),
    );
    Ok(parts.join("/"))
}

fn unique_export_path(export_dir: &Path) -> PathBuf {
    let timestamp = now_ms();
    let base_name = format!("remiss-logs-{timestamp}");
    let mut candidate = export_dir.join(format!("{base_name}.zip"));
    let mut suffix = 2usize;
    while candidate.exists() {
        candidate = export_dir.join(format!("{base_name}-{suffix}.zip"));
        suffix += 1;
    }
    candidate
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_are_rooted_under_remiss_logs() {
        let root = Path::new("/tmp/remiss/copilot-diagnostics");
        let path = root.join("latest.json");

        assert_eq!(
            archive_name_for(&path, root, "copilot-diagnostics").unwrap(),
            "remiss-logs/copilot-diagnostics/latest.json"
        );
    }

    #[test]
    fn unique_export_path_uses_zip_extension() {
        let path = unique_export_path(Path::new("/tmp"));
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("zip"));
    }
}
