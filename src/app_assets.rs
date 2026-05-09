use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use gpui::{AssetSource, Result, SharedString};
use lucide_icons::LUCIDE_FONT_BYTES;

pub const APP_MARK_ASSET: &str = "brand/app-icon.png";
pub const BRAND_HERO_LANDSCAPE_ASSET: &str = "brand/hero_landscape.png";

pub struct AppAssets {
    base: PathBuf,
}

impl AppAssets {
    pub fn new() -> Self {
        Self {
            base: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"),
        }
    }
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
