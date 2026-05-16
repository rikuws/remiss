use std::borrow::Cow;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use gpui::{AssetSource, Result, SharedString};
use lucide_icons::LUCIDE_FONT_BYTES;

pub const APP_LOGO_ASSET: &str = "brand/remiss-app-icon.png";

pub struct AppAssets {
    base: PathBuf,
}

impl AppAssets {
    pub fn new() -> Self {
        Self {
            base: bundled_assets_dir().unwrap_or_else(source_assets_dir),
        }
    }
}

fn source_assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

fn bundled_assets_dir() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    bundled_assets_dir_for_executable(&executable, |path| path.is_dir())
}

fn bundled_assets_dir_for_executable(
    executable: &Path,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(assets_dir) = macos_bundled_assets_dir(executable, &is_dir) {
        return Some(assets_dir);
    }

    let executable_dir = executable.parent()?;
    let assets_dir = executable_dir.join("assets");
    is_dir(&assets_dir).then_some(assets_dir)
}

fn macos_bundled_assets_dir(executable: &Path, is_dir: &impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let macos_dir = executable.parent()?;
    let contents_dir = macos_dir.parent()?;

    if macos_dir.file_name() != Some(OsStr::new("MacOS"))
        || contents_dir.file_name() != Some(OsStr::new("Contents"))
    {
        return None;
    }

    let assets_dir = contents_dir.join("Resources").join("assets");
    is_dir(&assets_dir).then_some(assets_dir)
}

pub fn load_bundled_fonts() -> Result<Vec<Cow<'static, [u8]>>> {
    let font_dir = AppAssets::new().base.join("fonts");
    let mut font_paths: Vec<_> = fs::read_dir(font_dir)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            match extension.as_str() {
                "otf" | "ttf" => Some(path),
                _ => None,
            }
        })
        .collect();
    font_paths.sort();

    let mut fonts: Vec<Cow<'static, [u8]>> = font_paths
        .into_iter()
        .map(|path| fs::read(path).map(Cow::Owned).map_err(Into::into))
        .collect::<Result<_>>()?;
    fonts.push(Cow::Borrowed(LUCIDE_FONT_BYTES));
    Ok(fonts)
}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(Cow::Owned(data)))
            .map_err(Into::into)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_assets_inside_macos_bundle() {
        let executable = Path::new("/Applications/Remiss.app/Contents/MacOS/remiss");
        let expected = Path::new("/Applications/Remiss.app/Contents/Resources/assets");
        let result = bundled_assets_dir_for_executable(executable, |path| path == expected);

        assert_eq!(result.as_deref(), Some(expected));
    }

    #[test]
    fn resolves_assets_next_to_packaged_executable() {
        let executable = Path::new("/tmp/Remiss/Remiss.exe");
        let expected = Path::new("/tmp/Remiss/assets");
        let result = bundled_assets_dir_for_executable(executable, |path| path == expected);

        assert_eq!(result.as_deref(), Some(expected));
    }
}
