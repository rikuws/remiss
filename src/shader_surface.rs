use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::*;
use serde::{Deserialize, Serialize};

use crate::cache::CacheStore;

const PROJECT_SHADER_SETTINGS_CACHE_KEY: &str = "project-shader-settings-v1";

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverviewShaderVariant {
    #[default]
    Bands,
    Ember,
    Ribbon,
    Interference,
}

impl OverviewShaderVariant {
    pub const ALL: [Self; 4] = [Self::Bands, Self::Ember, Self::Ribbon, Self::Interference];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Bands => "Bands",
            Self::Ember => "Ember",
            Self::Ribbon => "Ribbon",
            Self::Interference => "Interference",
        }
    }

    pub fn for_project(project: &str) -> Self {
        Self::ALL[stable_seed_index(project, Self::ALL.len())]
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectShaderSettings {
    #[serde(default)]
    pub projects: BTreeMap<String, OverviewShaderVariant>,
}

impl ProjectShaderSettings {
    pub fn shader_for_project(&self, project: &str) -> OverviewShaderVariant {
        self.projects
            .get(project)
            .copied()
            .unwrap_or_else(|| OverviewShaderVariant::for_project(project))
    }

    pub fn set_project_shader(&mut self, project: &str, variant: OverviewShaderVariant) {
        self.projects.insert(project.to_string(), variant);
    }
}

pub fn load_project_shader_settings(cache: &CacheStore) -> Result<ProjectShaderSettings, String> {
    Ok(cache
        .get::<ProjectShaderSettings>(PROJECT_SHADER_SETTINGS_CACHE_KEY)?
        .map(|document| document.value)
        .unwrap_or_default())
}

pub fn save_project_shader_settings(
    cache: &CacheStore,
    settings: &ProjectShaderSettings,
) -> Result<(), String> {
    cache.put(PROJECT_SHADER_SETTINGS_CACHE_KEY, settings, now_ms())
}

fn stable_seed_index(seed: &str, len: usize) -> usize {
    let hash = seed.bytes().fold(2166136261u32, |acc, byte| {
        acc.wrapping_mul(16777619) ^ byte as u32
    });
    (hash as usize) % len
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ShaderCornerMask {
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_right: bool,
    pub bottom_left: bool,
}

impl ShaderCornerMask {
    pub const LEFT: Self = Self {
        top_left: true,
        top_right: false,
        bottom_right: false,
        bottom_left: true,
    };

    pub const TOP: Self = Self {
        top_left: true,
        top_right: true,
        bottom_right: false,
        bottom_left: false,
    };

    pub const ALL: Self = Self {
        top_left: true,
        top_right: true,
        bottom_right: true,
        bottom_left: true,
    };

    const fn any(self) -> bool {
        self.top_left || self.top_right || self.bottom_right || self.bottom_left
    }
}

pub fn opengl_shader_surface(seed: impl Into<String>) -> Div {
    opengl_shader_surface_variant(seed, OverviewShaderVariant::Bands)
}

pub fn opengl_shader_surface_variant(
    seed: impl Into<String>,
    variant: OverviewShaderVariant,
) -> Div {
    platform::shader_surface(seed.into(), variant)
}

pub fn opengl_shader_surface_with_corner_mask(
    seed: impl Into<String>,
    radius: Pixels,
    mask_color: Rgba,
    corners: ShaderCornerMask,
) -> Div {
    opengl_shader_surface_variant_with_corner_mask(
        seed,
        OverviewShaderVariant::Bands,
        radius,
        mask_color,
        corners,
    )
}

pub fn opengl_shader_surface_variant_with_corner_mask(
    seed: impl Into<String>,
    variant: OverviewShaderVariant,
    radius: Pixels,
    mask_color: Rgba,
    corners: ShaderCornerMask,
) -> Div {
    // GPUI's overflow mask is rectangular, so the CVPixelBuffer still needs
    // the painted corner mask. Rounding the wrapper separately prevents its
    // fallback/background layer from showing through as square corners.
    let mut surface = platform::shader_surface(seed.into(), variant).rounded(radius);
    if corners.any() {
        surface = surface.child(shader_corner_mask(radius, mask_color, corners));
    }
    surface
}

fn shader_corner_mask(
    radius: Pixels,
    mask_color: Rgba,
    corners: ShaderCornerMask,
) -> impl IntoElement {
    canvas(
        move |_, _, _| (),
        move |bounds, _, window, _| {
            paint_shader_corner_mask(window, bounds, radius, mask_color, corners);
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn paint_shader_corner_mask(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    radius: Pixels,
    color: Rgba,
    corners: ShaderCornerMask,
) {
    let radius = f32::from(radius)
        .min(f32::from(bounds.size.width) / 2.0)
        .min(f32::from(bounds.size.height) / 2.0);
    if radius <= 0.0 {
        return;
    }

    let radius = px(radius);
    let control = px(f32::from(radius) * 0.552_284_8);
    let left = bounds.left();
    let right = bounds.right();
    let top = bounds.top();
    let bottom = bounds.bottom();

    if corners.top_left {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(left, top));
        builder.line_to(point(left + radius, top));
        builder.cubic_bezier_to(
            point(left, top + radius),
            point(left + radius - control, top),
            point(left, top + radius - control),
        );
        builder.line_to(point(left, top));
        builder.close();
        paint_mask_path(window, builder, color);
    }

    if corners.top_right {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(right, top));
        builder.line_to(point(right - radius, top));
        builder.cubic_bezier_to(
            point(right, top + radius),
            point(right - radius + control, top),
            point(right, top + radius - control),
        );
        builder.line_to(point(right, top));
        builder.close();
        paint_mask_path(window, builder, color);
    }

    if corners.bottom_right {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(right, bottom));
        builder.line_to(point(right, bottom - radius));
        builder.cubic_bezier_to(
            point(right - radius, bottom),
            point(right, bottom - radius + control),
            point(right - radius + control, bottom),
        );
        builder.line_to(point(right, bottom));
        builder.close();
        paint_mask_path(window, builder, color);
    }

    if corners.bottom_left {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(left, bottom));
        builder.line_to(point(left, bottom - radius));
        builder.cubic_bezier_to(
            point(left + radius, bottom),
            point(left, bottom - radius + control),
            point(left + radius - control, bottom),
        );
        builder.line_to(point(left, bottom));
        builder.close();
        paint_mask_path(window, builder, color);
    }
}

fn paint_mask_path(window: &mut Window, builder: PathBuilder, color: Rgba) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::{OverviewShaderVariant, ProjectShaderSettings};

    #[test]
    fn project_shader_fallback_is_stable() {
        let first = OverviewShaderVariant::for_project("rikuws/gh-ui");
        let second = OverviewShaderVariant::for_project("rikuws/gh-ui");

        assert_eq!(first, second);
        assert!(OverviewShaderVariant::ALL.contains(&first));
    }

    #[test]
    fn project_shader_settings_override_fallback() {
        let mut settings = ProjectShaderSettings::default();
        let fallback = settings.shader_for_project("rikuws/gh-ui");

        settings.set_project_shader("rikuws/gh-ui", OverviewShaderVariant::Ribbon);

        assert_eq!(
            settings.shader_for_project("rikuws/gh-ui"),
            OverviewShaderVariant::Ribbon
        );
        assert_eq!(
            ProjectShaderSettings::default().shader_for_project("rikuws/gh-ui"),
            fallback
        );
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::OverviewShaderVariant;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::OnceLock;
    use std::time::Instant;

    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use core_video::pixel_buffer::{
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, CVPixelBuffer, CVPixelBufferKeys,
    };
    use gpui::prelude::*;
    use gpui::*;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
    use objc2_core_image::{
        kCIContextUseSoftwareRenderer, CIColorKernel, CIContext, CIContextOption, CIImage, CIVector,
    };
    use objc2_core_video::CVPixelBuffer as ObjcCVPixelBuffer;
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

    const TARGET_FRAME_RATE: f32 = 30.0;

    const BANDS_CORE_IMAGE_SHADER: &str = r#"
kernel vec4 overviewShader(float iTime, vec2 iResolution) {
    vec2 fragCoord = destCoord();
    vec2 uv = fragCoord / iResolution;

    float d = -(iTime * 0.3);
    float a = 0.0;

    for (float i = 0.0; i < 9.0; ++i) {
        a += cos(d + i * uv.x - a);
        d += 0.5 * sin(a + i * uv.y);
    }

    d += iTime * 0.3;

    float r = cos(uv.x * a) * 0.7 + 0.3;
    float g = cos(uv.y * d) * 0.5 + 0.2;
    float b = cos(a + d) * 0.3 + 0.5;
    vec3 col = vec3(r, g, b);
    col = cos(col * cos(vec3(d, a, 2.5)) * 0.5 + 0.5);

    return vec4(col, 1.0);
}
"#;

    const EMBER_CORE_IMAGE_SHADER: &str = r#"
kernel vec4 overviewShader(float iTime, vec2 iResolution) {
    vec2 fragCoord = destCoord();
    vec2 uv = fragCoord / iResolution;

    float t = iTime * 0.14;
    float d = -t;
    float a = 0.0;

    for (float i = 0.0; i < 9.0; ++i) {
        a += cos(d + i * uv.x - a);
        d += 0.5 * sin(a + i * uv.y);
    }

    d += t;

    float r = cos(uv.x * a) * 0.7 + 0.3;
    float g = cos(uv.y * d) * 0.5 + 0.2;
    float b = cos(a + d) * 0.3 + 0.5;
    vec3 col = vec3(r, g, b);
    col = cos(col * cos(vec3(d, a, 2.5)) * 0.5 + 0.5);
    col = vec3(
        col.r * 0.68 + col.g * 0.12 + 0.16,
        col.g * 0.58 + col.b * 0.14 + 0.08,
        col.b * 0.62 + col.r * 0.10 + 0.10
    );
    col = min(max(col, vec3(0.0)), vec3(1.0));

    return vec4(col, 1.0);
}
"#;

    const RIBBON_CORE_IMAGE_SHADER: &str = r#"
kernel vec4 overviewShader(float iTime, vec2 iResolution) {
    vec2 uv = destCoord() / iResolution;
    float x = uv.x;
    float y = uv.y;
    float t = iTime * 0.18;

    float sweep = 0.5 + 0.5 * sin((x * 0.82 + y * 0.18 - t * 0.34) * 6.2831853);
    vec3 col = vec3(0.58, 0.76, 1.0) + (vec3(0.00, 0.94, 0.80) - vec3(0.58, 0.76, 1.0)) * sweep;

    float c1 = 0.33 + 0.15 * sin((x * 1.12 - t) * 6.2831853) + 0.04 * sin((x * 3.40 + t * 1.10) * 6.2831853);
    float c2 = 0.64 + 0.17 * sin((x * 0.96 + t * 0.74) * 6.2831853) + 0.05 * sin((x * 2.85 - t * 1.35) * 6.2831853);
    float c3 = 0.08 + 0.13 * sin((x * 1.42 - t * 0.82) * 6.2831853);

    float band1 = 1.0 - smoothstep(0.045, 0.180, abs(y - c1));
    float band2 = 1.0 - smoothstep(0.055, 0.210, abs(y - c2));
    float band3 = 1.0 - smoothstep(0.030, 0.135, abs(y - c3));
    float bands = min(max(band1 * 0.72 + band2 * 0.84 + band3 * 0.42, 0.0), 1.0);

    col = col + (vec3(0.92, 0.90, 1.00) - col) * bands;

    float mintEdge = 1.0 - smoothstep(0.000, 0.032, abs(y - c2 + 0.125));
    float limeEdge = 1.0 - smoothstep(0.000, 0.026, abs(y - c1 - 0.145));
    col = col + (vec3(0.62, 1.00, 0.62) - col) * min(max(mintEdge * 0.58 + limeEdge * 0.36, 0.0), 1.0);

    float rightLight = 1.0 - smoothstep(0.0, 0.45, length((uv - vec2(1.03, 0.37 + 0.10 * sin(t * 6.2831853))) * vec2(0.9, 1.5)));
    col = col + (vec3(0.56, 1.00, 0.68) - col) * (rightLight * 0.38);
    col = min(max(col, vec3(0.0)), vec3(1.0));

    return vec4(col, 1.0);
}
"#;

    const INTERFERENCE_CORE_IMAGE_SHADER: &str = r#"
kernel vec4 overviewShader(float iTime, vec2 iResolution) {
    vec2 uv = destCoord() / iResolution;
    float x = uv.x;
    float y = uv.y;
    float t = iTime * 0.16;

    vec3 col = vec3(0.30, 0.72, 0.95);

    vec2 p = uv - vec2(0.78 + 0.05 * sin(t * 4.4), 0.62 + 0.08 * cos(t * 3.2));
    float redBloom = 1.0 - smoothstep(0.0, 0.74, length(p * vec2(1.00, 1.24)));
    col = col + (vec3(1.00, 0.22, 0.18) - col) * (redBloom * 0.82);

    p = uv - vec2(0.34 + 0.04 * cos(t * 5.1), 0.24 + 0.05 * sin(t * 4.7));
    float goldBloom = 1.0 - smoothstep(0.0, 0.64, length(p * vec2(1.18, 0.90)));
    col = col + (vec3(1.00, 0.66, 0.16) - col) * (goldBloom * 0.64);

    float wave = x * 25.5 + 0.44 * sin(y * 6.4 + t * 6.2831853) + 0.10 * sin(y * 18.0 - t * 8.0);
    float stripe = 0.5 + 0.5 * cos(wave * 6.2831853);
    stripe = stripe * stripe * stripe * stripe;

    float curtain = smoothstep(0.06, 0.82, stripe);
    float reach = 0.58 + 0.42 * sin((y * 1.65 - x * 0.52 + t * 0.90) * 6.2831853);
    curtain *= 0.72 + 0.28 * reach;

    float coolStripe = smoothstep(0.28, 0.95, 1.0 - stripe);
    col = col + (vec3(0.23, 0.70, 0.96) - col) * (coolStripe * 0.34);
    col = col + (vec3(1.00, 0.72, 0.20) - col) * (curtain * 0.54);

    float diagonalHeat = 1.0 - smoothstep(0.06, 0.34, abs(y - (0.16 + x * 0.52 + 0.05 * sin((x * 2.0 + t) * 6.2831853))));
    col = col + (vec3(1.00, 0.35, 0.20) - col) * (diagonalHeat * 0.36);
    col = min(max(col, vec3(0.0)), vec3(1.0));

    return vec4(col, 1.0);
}
"#;

    thread_local! {
        static TARGETS: RefCell<HashMap<ShaderTargetKey, ShaderTarget>> = RefCell::new(HashMap::new());
    }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct ShaderTargetKey {
        seed: String,
        variant: OverviewShaderVariant,
        width: usize,
        height: usize,
    }

    struct ShaderGpu {
        context: Retained<CIContext>,
        bands_kernel: Retained<CIColorKernel>,
        ember_kernel: Retained<CIColorKernel>,
        ribbon_kernel: Retained<CIColorKernel>,
        interference_kernel: Retained<CIColorKernel>,
    }

    struct ShaderTarget {
        buffer: CVPixelBuffer,
        frame_bucket: Option<u64>,
    }

    pub fn shader_surface(seed: String, variant: OverviewShaderVariant) -> Div {
        div().relative().overflow_hidden().bg(rgb(0x06111f)).child(
            canvas(
                {
                    let seed = seed.clone();
                    move |bounds, window, _| {
                        render_shader_target(bounds, window.scale_factor(), &seed, variant)
                    }
                },
                move |bounds, target, window, _| {
                    if let Some(target) = target {
                        window.paint_surface(bounds, target);
                    } else {
                        window.paint_quad(fill(bounds, rgb(0x06111f)));
                    }
                    window.request_animation_frame();
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
    }

    fn render_shader_target(
        bounds: Bounds<Pixels>,
        scale_factor: f32,
        seed: &str,
        variant: OverviewShaderVariant,
    ) -> Option<CVPixelBuffer> {
        let width = even_device_pixels(f32::from(bounds.size.width) * scale_factor);
        let height = even_device_pixels(f32::from(bounds.size.height) * scale_factor);
        let key = ShaderTargetKey {
            seed: seed.to_string(),
            variant,
            width,
            height,
        };

        let elapsed = shader_elapsed();
        let frame_bucket = (elapsed * TARGET_FRAME_RATE).floor() as u64;
        let (target, last_frame_bucket) = TARGETS.with(|targets| {
            let mut targets = targets.borrow_mut();
            if targets.len() > 160 {
                targets.retain(|cached_key, _| cached_key == &key);
            }

            if let Some(target) = targets.get(&key) {
                return Some((target.buffer.clone(), target.frame_bucket));
            }

            let buffer = create_target(width, height)?;
            targets.insert(
                key.clone(),
                ShaderTarget {
                    buffer: buffer.clone(),
                    frame_bucket: None,
                },
            );
            Some((buffer, None))
        })?;

        if last_frame_bucket != Some(frame_bucket) {
            let time = frame_bucket as f32 / TARGET_FRAME_RATE + seed_phase(seed);
            shader_gpu().render(&target, width, height, time, variant)?;
            TARGETS.with(|targets| {
                if let Some(target) = targets.borrow_mut().get_mut(&key) {
                    target.frame_bucket = Some(frame_bucket);
                }
            });
        }

        Some(target)
    }

    fn create_target(width: usize, height: usize) -> Option<CVPixelBuffer> {
        let iosurface_properties = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[]);
        let options = CFDictionary::from_CFType_pairs(&[
            (
                CFString::from(CVPixelBufferKeys::MetalCompatibility),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::from(CVPixelBufferKeys::IOSurfaceProperties),
                iosurface_properties.as_CFType(),
            ),
        ]);

        CVPixelBuffer::new(
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            width,
            height,
            Some(&options),
        )
        .ok()
    }

    fn shader_gpu() -> &'static ShaderGpu {
        static GPU: OnceLock<ShaderGpu> = OnceLock::new();
        GPU.get_or_init(|| {
            let software_renderer: Retained<AnyObject> = NSNumber::new_bool(false).into();
            let options: Retained<NSDictionary<CIContextOption, AnyObject>> =
                // SAFETY: `kCIContextUseSoftwareRenderer` is Core Image's
                // documented context option key for a boolean NSNumber value.
                NSDictionary::from_slices(
                    &[unsafe { kCIContextUseSoftwareRenderer }],
                    &[&*software_renderer],
                );
            let bands_source = NSString::from_str(BANDS_CORE_IMAGE_SHADER);
            let ember_source = NSString::from_str(EMBER_CORE_IMAGE_SHADER);
            let ribbon_source = NSString::from_str(RIBBON_CORE_IMAGE_SHADER);
            let interference_source = NSString::from_str(INTERFERENCE_CORE_IMAGE_SHADER);
            // SAFETY: These static shader sources are compiled once during
            // startup, and Core Image reports invalid kernels via `None`.
            #[allow(deprecated)]
            let bands_kernel = unsafe { CIColorKernel::kernelWithString(&bands_source) }
                .expect("overview bands shader must compile as a Core Image GPU kernel");
            #[allow(deprecated)]
            let ember_kernel = unsafe { CIColorKernel::kernelWithString(&ember_source) }
                .expect("overview ember shader must compile as a Core Image GPU kernel");
            #[allow(deprecated)]
            let ribbon_kernel = unsafe { CIColorKernel::kernelWithString(&ribbon_source) }
                .expect("overview ribbon shader must compile as a Core Image GPU kernel");
            #[allow(deprecated)]
            let interference_kernel =
                unsafe { CIColorKernel::kernelWithString(&interference_source) }
                    .expect("overview interference shader must compile as a Core Image GPU kernel");
            // SAFETY: `options` only contains the documented boolean renderer
            // preference and stays alive for the duration of this call.
            let context = unsafe { CIContext::contextWithOptions(Some(&options)) };

            ShaderGpu {
                context,
                bands_kernel,
                ember_kernel,
                ribbon_kernel,
                interference_kernel,
            }
        })
    }

    impl ShaderGpu {
        fn render(
            &self,
            target: &CVPixelBuffer,
            width: usize,
            height: usize,
            time: f32,
            variant: OverviewShaderVariant,
        ) -> Option<Retained<CIImage>> {
            let time: Retained<AnyObject> = NSNumber::new_f32(time).into();
            let resolution: Retained<AnyObject> =
                // SAFETY: CIVector copies these scalar coordinates into a new
                // Objective-C value object; the Rust inputs remain valid.
                unsafe { CIVector::vectorWithX_Y(width as CGFloat, height as CGFloat).into() };
            let args = NSArray::from_retained_slice(&[time, resolution]);
            let extent = CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(width as CGFloat, height as CGFloat),
            );
            // SAFETY: `args` matches the kernel signatures compiled above and
            // `extent` bounds the render area for the returned CIImage.
            let image = unsafe {
                self.kernel(variant)
                    .applyWithExtent_arguments(extent, &args)?
            };
            // SAFETY: `target` is a live CVPixelBuffer allocated by this module,
            // and `image` stays retained for the duration of the render call.
            unsafe {
                self.context
                    .render_toCVPixelBuffer(&image, objc_cv_pixel_buffer(target));
            }

            Some(image)
        }

        fn kernel(&self, variant: OverviewShaderVariant) -> &CIColorKernel {
            match variant {
                OverviewShaderVariant::Bands => &self.bands_kernel,
                OverviewShaderVariant::Ember => &self.ember_kernel,
                OverviewShaderVariant::Ribbon => &self.ribbon_kernel,
                OverviewShaderVariant::Interference => &self.interference_kernel,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use objc2_core_image::CIColorKernel;
        use objc2_foundation::NSString;

        #[::core::prelude::v1::test]
        fn overview_shader_kernels_compile() {
            let shaders = [
                ("bands", super::BANDS_CORE_IMAGE_SHADER),
                ("ember", super::EMBER_CORE_IMAGE_SHADER),
                ("ribbon", super::RIBBON_CORE_IMAGE_SHADER),
                ("interference", super::INTERFERENCE_CORE_IMAGE_SHADER),
            ];

            for (name, source) in shaders {
                let source = NSString::from_str(source);
                // SAFETY: The test uses the same static shader source strings
                // that production initializes, and only checks compilation.
                #[allow(deprecated)]
                let kernel = unsafe { CIColorKernel::kernelWithString(&source) };
                assert!(kernel.is_some(), "{name} shader should compile");
            }
        }
    }

    fn even_device_pixels(value: f32) -> usize {
        let pixels = value.ceil().max(2.0) as usize;
        pixels + pixels % 2
    }

    // SAFETY: `CVPixelBuffer` and `ObjcCVPixelBuffer` are two typed views over
    // the same CoreVideo object. The returned reference borrows `buffer`'s
    // lifetime and is only used for the immediate Core Image render call.
    unsafe fn objc_cv_pixel_buffer(buffer: &CVPixelBuffer) -> &ObjcCVPixelBuffer {
        &*(buffer.as_concrete_TypeRef() as *const ObjcCVPixelBuffer)
    }

    fn shader_elapsed() -> f32 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f32()
    }

    fn seed_phase(seed: &str) -> f32 {
        let hash = seed.bytes().fold(2166136261u32, |acc, byte| {
            acc.wrapping_mul(16777619) ^ byte as u32
        });
        hash as f32 / u32::MAX as f32 * 7.0
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::OverviewShaderVariant;
    use gpui::prelude::*;
    use gpui::*;

    pub fn shader_surface(_seed: String, _variant: OverviewShaderVariant) -> Div {
        div().bg(rgb(0x06111f))
    }
}
