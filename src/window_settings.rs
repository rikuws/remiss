use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{px, size, Pixels, Size};
use serde::{Deserialize, Serialize};

use crate::cache::CacheStore;

const WINDOW_SIZE_CACHE_KEY: &str = "app.window.size";
const DEFAULT_WINDOW_WIDTH: f32 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 800.0;
const MIN_WINDOW_WIDTH: f32 = 800.0;
const MIN_WINDOW_HEIGHT: f32 = 500.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct WindowSizeSettings {
    width: f32,
    height: f32,
}

pub fn default_window_size() -> Size<Pixels> {
    size(px(DEFAULT_WINDOW_WIDTH), px(DEFAULT_WINDOW_HEIGHT))
}

pub fn load_window_size(cache: &CacheStore) -> Size<Pixels> {
    cache
        .get::<WindowSizeSettings>(WINDOW_SIZE_CACHE_KEY)
        .ok()
        .flatten()
        .and_then(|document| validate_window_size(document.value))
        .unwrap_or_else(default_window_size)
}

pub fn save_window_size(cache: &CacheStore, window_size: Size<Pixels>) -> Result<(), String> {
    let Some(settings) = window_size_settings(window_size) else {
        return Ok(());
    };

    cache.put(WINDOW_SIZE_CACHE_KEY, &settings, now_ms())
}

fn validate_window_size(settings: WindowSizeSettings) -> Option<Size<Pixels>> {
    if !is_valid_dimension(settings.width, MIN_WINDOW_WIDTH)
        || !is_valid_dimension(settings.height, MIN_WINDOW_HEIGHT)
    {
        return None;
    }

    Some(size(px(settings.width), px(settings.height)))
}

fn window_size_settings(window_size: Size<Pixels>) -> Option<WindowSizeSettings> {
    let width = f32::from(window_size.width);
    let height = f32::from(window_size.height);
    let settings = WindowSizeSettings { width, height };

    validate_window_size(settings).map(|_| settings)
}

fn is_valid_dimension(value: f32, minimum: f32) -> bool {
    value.is_finite() && value >= minimum
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_cache() -> CacheStore {
        CacheStore::new(unique_test_path("window-settings-cache.sqlite3"))
            .expect("failed to create temp cache")
    }

    fn unique_test_path(file_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let test_id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "remiss-window-settings-{nanos}-{test_id}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("failed to create temp directory");
        dir.join(file_name)
    }

    fn assert_window_size(window_size: Size<Pixels>, width: f32, height: f32) {
        assert_eq!(f32::from(window_size.width), width);
        assert_eq!(f32::from(window_size.height), height);
    }

    #[test]
    fn missing_cache_returns_default_window_size() {
        let cache = temp_cache();

        assert_window_size(
            load_window_size(&cache),
            DEFAULT_WINDOW_WIDTH,
            DEFAULT_WINDOW_HEIGHT,
        );
    }

    #[test]
    fn saved_valid_size_is_returned() {
        let cache = temp_cache();

        save_window_size(&cache, size(px(1440.0), px(900.0))).expect("failed to save size");

        assert_window_size(load_window_size(&cache), 1440.0, 900.0);
    }

    #[test]
    fn invalid_tiny_size_returns_default_window_size() {
        let cache = temp_cache();
        cache
            .put(
                WINDOW_SIZE_CACHE_KEY,
                &WindowSizeSettings {
                    width: 320.0,
                    height: 240.0,
                },
                1,
            )
            .expect("failed to save tiny size");

        assert_window_size(
            load_window_size(&cache),
            DEFAULT_WINDOW_WIDTH,
            DEFAULT_WINDOW_HEIGHT,
        );
    }
}
