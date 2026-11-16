//! Resident images, and the decodes behind them.
//!
//! egui keeps a GPU texture for every URI it has loaded and, unless asked,
//! the file bytes and the decoded pixels too. [`crate::theme::install`] turns
//! on `reduce_texture_memory`, and [`sweep`] forgets images that have scrolled
//! away once their decoded RGBA and a matching GPU texture would pass
//! [`BYTE_BUDGET`].
//!
//! Inline photos, thumbnails and avatars are decoded at the size they are
//! shown. The size hint egui passes is already in physical pixels, so a HiDPI
//! scale is included. The reported source size stays the file's own, so
//! layout, cropping and zoom keep using the original aspect and pixel count.
//! The image viewer loads those original pixels under a separate URI and
//! drops that texture when the viewer is not on screen.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use egui::load::{BytesLoader as _, ImageLoader, ImagePoll, LoadError, SizeHint, TextureLoader};
use egui::{ColorImage, Context, TextureHandle, TextureOptions, Vec2};

/// How many bytes of decoded images stay resident.
///
/// Each image counts its decoded RGBA buffer and the same number of bytes
/// again for the GPU texture (RGBA8, no mipmaps). egui drops the buffer once
/// the texture is uploaded, so a full budget is about half of this in
/// steady-state GPU memory, with the other half reserved for the copy that
/// exists while a texture is created. 48 MiB holds twenty-four 512×512 images
/// counted this way, against ninety-six under the previous item cap.
pub const BYTE_BUDGET: usize = 48 * 1024 * 1024;

/// RGBA bytes of a decoded buffer plus the same estimate for its GPU texture.
const fn retained_bytes(width: u32, height: u32) -> usize {
    let pixels = (width as usize).saturating_mul(height as usize);
    pixels.saturating_mul(4).saturating_mul(2)
}

/// Prefix of the viewer's URI. The texture cache keys on the whole URI and
/// keeps one raster per URI, so the full-resolution decode cannot share the
/// URI of the display-sized one.
const FULL_PREFIX: &str = "zapfast-full:";

/// Pixel step for a display decode. A one-pixel resize stays in the bucket it
/// already filled, and a later, larger request replaces that texture.
const SIZE_STEP: u32 = 64;

struct Entry {
    /// Frame the image was last drawn on.
    frame: u64,
    /// Whether its bytes are registered with egui's byte loader. egui drops
    /// them once the texture is uploaded, so this is what says a later draw
    /// has to register them again.
    registered: bool,
    /// Decoded RGBA bytes plus the GPU texture estimate. Zero until the
    /// texture exists.
    bytes: usize,
}

#[derive(Clone, Default)]
struct Registry(Arc<Mutex<HashMap<String, Entry>>>);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn registry(ctx: &Context) -> Registry {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Registry>(egui::Id::new("image-cache"))
            .clone()
    })
}

fn budget_id() -> egui::Id {
    egui::Id::new("image-cache-budget")
}

fn viewer_id() -> egui::Id {
    egui::Id::new("image-cache-viewer")
}

#[derive(Clone, Copy)]
struct BudgetOverride(usize);

fn budget_of(ctx: &Context) -> usize {
    ctx.data(|data| data.get_temp::<BudgetOverride>(budget_id()))
        .map(|budget| budget.0)
        .unwrap_or(BYTE_BUDGET)
}

/// Sum of the retained-byte estimates currently charged to the cache.
#[cfg(test)]
fn retained(ctx: &Context) -> usize {
    let registry = registry(ctx);
    lock(&registry.0).values().map(|entry| entry.bytes).sum()
}

/// Records the decoded size of `uri` for the next [`sweep`].
fn note_retained(ctx: &Context, uri: &str, width: u32, height: u32) {
    let bytes = retained_bytes(width, height);
    let frame = ctx.cumulative_frame_nr();
    let registry = registry(ctx);
    let mut entries = lock(&registry.0);
    match entries.get_mut(uri) {
        Some(entry) => entry.bytes = bytes,
        None => {
            entries.insert(
                uri.to_owned(),
                Entry {
                    frame,
                    registered: false,
                    bytes,
                },
            );
        }
    }
}

/// Gives a test a smaller budget than [`BYTE_BUDGET`].
#[cfg(test)]
fn set_budget(ctx: &Context, bytes: usize) {
    ctx.data_mut(|data| data.insert_temp(budget_id(), BudgetOverride(bytes)));
}

/// Charges `uri` as if it decoded to `bytes`, for tests that do not upload.
#[cfg(test)]
fn charge(ctx: &Context, uri: &str, bytes: usize) {
    let registry = registry(ctx);
    let mut entries = lock(&registry.0);
    match entries.get_mut(uri) {
        Some(entry) => entry.bytes = bytes,
        None => {
            entries.insert(
                uri.to_owned(),
                Entry {
                    frame: ctx.cumulative_frame_nr(),
                    registered: false,
                    bytes,
                },
            );
        }
    }
}

/// URI of the viewer's full-resolution decode of `path`.
pub(crate) fn full_uri(path: &std::path::Path) -> String {
    format!("{FULL_PREFIX}{}", crate::util::image_uri(path))
}

/// The viewer's image: the file's own pixels, tracked so [`sweep`] keeps it
/// while it is on screen and [`release_viewer`] can drop it afterwards.
pub(crate) fn viewer_image(ui: &egui::Ui, path: &std::path::Path) -> egui::Image<'static> {
    let uri = full_uri(path);
    let ctx = ui.ctx();
    let previous = ctx.data(|data| data.get_temp::<String>(viewer_id()));
    if previous.as_deref() != Some(uri.as_str()) {
        if let Some(previous) = previous {
            forget(ctx, &previous);
        }
        ctx.data_mut(|data| data.insert_temp(viewer_id(), uri.clone()));
    }
    touch(ctx, &uri);
    egui::Image::new(uri)
}

/// Drops the viewer's full-resolution texture, if one is loaded.
pub(crate) fn release_viewer(ctx: &Context) {
    let Some(uri) = ctx.data(|data| data.get_temp::<String>(viewer_id())) else {
        return;
    };
    ctx.data_mut(|data| {
        data.remove::<String>(viewer_id());
    });
    forget(ctx, &uri);
}

/// Registers `bytes` under `uri`, and marks the image as drawn this frame.
///
/// Re-registers them after [`sweep`] has forgotten the image, or after egui
/// has dropped them once the texture was uploaded.
pub fn include(ctx: &Context, uri: &str, bytes: &[u8]) {
    let frame = ctx.cumulative_frame_nr();
    let registry = registry(ctx);
    let mut entries = lock(&registry.0);
    let fresh = match entries.get_mut(uri) {
        Some(entry) => {
            entry.frame = frame;
            !std::mem::replace(&mut entry.registered, true)
        }
        None => {
            entries.insert(
                uri.to_owned(),
                Entry {
                    frame,
                    registered: true,
                    bytes: 0,
                },
            );
            true
        }
    };
    drop(entries);
    if fresh {
        ctx.include_bytes(uri.to_owned(), bytes.to_vec());
    }
}

/// Marks an image URI as drawn this frame.
pub fn touch(ctx: &Context, uri: &str) {
    let frame = ctx.cumulative_frame_nr();
    let registry = registry(ctx);
    let mut entries = lock(&registry.0);
    match entries.get_mut(uri) {
        Some(entry) => entry.frame = frame,
        None => {
            entries.insert(
                uri.to_owned(),
                Entry {
                    frame,
                    registered: false,
                    bytes: 0,
                },
            );
        }
    }
}

/// Forgets one image now rather than once it falls out of the budget: the
/// info panel frees its thumbnails when it closes, and the viewer frees its
/// full-resolution texture. A later draw registers the bytes again.
pub fn forget(ctx: &Context, uri: &str) {
    let registry = registry(ctx);
    lock(&registry.0).remove(uri);
    ctx.forget_image(uri);
}

/// Forgets the images that have not been drawn for the longest, until the
/// ones that remain fit in [`BYTE_BUDGET`].
///
/// Images the current frame drew are never forgotten, so a frame that shows
/// more than the budget does not reload them on the next one. A full-resolution
/// photo in the viewer is such an image: it stays while it is open, even when
/// it is larger than the budget, and leaves when the viewer does.
pub fn sweep(ctx: &Context) {
    let frame = ctx.cumulative_frame_nr();
    let budget = budget_of(ctx);
    let stale: Vec<String> = {
        let registry = registry(ctx);
        let mut entries = lock(&registry.0);
        let mut held: usize = entries.values().map(|entry| entry.bytes).sum();
        if held <= budget {
            return;
        }
        let mut idle: Vec<(u64, String)> = entries
            .iter()
            .filter(|(_, entry)| entry.frame < frame)
            .map(|(uri, entry)| (entry.frame, uri.clone()))
            .collect();
        idle.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        let mut stale = Vec::new();
        for (_, uri) in idle {
            if held <= budget {
                break;
            }
            let Some(entry) = entries.remove(&uri) else {
                continue;
            };
            held = held.saturating_sub(entry.bytes);
            stale.push(uri);
        }
        stale
    };
    for uri in stale {
        ctx.forget_image(&uri);
    }
}

/// Installs the display-size decoder ahead of egui's own loaders.
///
/// Called once per context, after `egui_extras::install_image_loaders`, so a
/// raster egui would have decoded at full resolution is decoded here instead.
/// SVG icons and GIF animations are left to those loaders.
pub(crate) fn install(ctx: &Context) {
    let present = ctx
        .loaders()
        .image
        .lock()
        .iter()
        .any(|loader| loader.id() == Decoded::ID);
    if present {
        return;
    }
    let cache = Arc::new(Mutex::new(Images::default()));
    ctx.add_image_loader(Arc::new(Decoded {
        cache: Arc::clone(&cache),
    }));
    ctx.add_texture_loader(Arc::new(Uploaded { cache }));
}

/// Drops egui's `#<frame>` suffix. Still WebP files are requested that way
/// (`photo.webp#0`) even when nothing is animated; the file itself has no
/// such name.
fn strip_frame(uri: &str) -> &str {
    match egui::decode_animated_image_uri(uri) {
        Ok((bare, _)) => bare,
        Err(_) => uri,
    }
}

/// The bytes a URI is loaded from: no viewer prefix and no frame suffix.
fn source_uri(uri: &str) -> &str {
    strip_frame(uri.strip_prefix(FULL_PREFIX).unwrap_or(uri))
}

/// The URI the interface registered. The viewer's prefix stays, so its
/// texture is not the display-sized one, and the frame suffix does not.
fn identity(uri: &str) -> String {
    let (prefix, rest) = uri
        .strip_prefix(FULL_PREFIX)
        .map_or(("", uri), |rest| (FULL_PREFIX, rest));
    format!("{prefix}{}", strip_frame(rest))
}

/// Still rasters this module decodes. SVG icons and GIFs belong to the
/// loaders installed before this one: icons must keep their bytes (they are
/// drawn at several sizes), and a GIF is a frame sequence.
fn claims(uri: &str) -> bool {
    let source = source_uri(uri);
    if egui::load::has_extension(source, "svg") || egui::load::has_extension(source, "gif") {
        return false;
    }
    source.starts_with("file://") || source.starts_with("bytes://")
}

fn quantize(value: u32) -> u32 {
    let value = value.max(1);
    if value > u32::MAX - (SIZE_STEP - 1) {
        return value;
    }
    value.div_ceil(SIZE_STEP) * SIZE_STEP
}

/// The pixel box a load should fill. `Full` is the file's own resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Target {
    Full,
    Fit { width: u32, height: u32 },
}

fn target_for(uri: &str, hint: SizeHint) -> Target {
    if uri.starts_with(FULL_PREFIX) {
        return Target::Full;
    }
    match hint {
        SizeHint::Scale(_) => Target::Full,
        SizeHint::Width(width) => Target::Fit {
            width: quantize(width),
            height: u32::MAX,
        },
        SizeHint::Height(height) => Target::Fit {
            width: u32::MAX,
            height: quantize(height),
        },
        SizeHint::Size { width, height, .. } => Target::Fit {
            width: quantize(width),
            height: quantize(height),
        },
    }
}

/// Fits `width`×`height` inside the box without enlarging it.
fn fit_dimensions(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let max_width = max_width.max(1);
    let max_height = max_height.max(1);
    if width <= max_width && height <= max_height {
        return (width, height);
    }
    let scale = (max_width as f32 / width as f32).min(max_height as f32 / height as f32);
    let fitted = |value: u32, max: u32| -> u32 {
        let scaled = (value as f32 * scale).round();
        if !scaled.is_finite() || scaled <= 1.0 {
            return 1;
        }
        if scaled >= max as f32 {
            return max;
        }
        scaled as u32
    };
    (fitted(width, max_width), fitted(height, max_height))
}

fn decode(bytes: &[u8], target: Target) -> Result<ColorImage, String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    if reader.format().is_none() {
        return Err("unsupported image".to_owned());
    }
    let decoded = reader.decode().map_err(|err| err.to_string())?;
    let original_width = decoded.width();
    let original_height = decoded.height();
    let (width, height) = match target {
        Target::Full => (original_width.max(1), original_height.max(1)),
        Target::Fit {
            width: max_width,
            height: max_height,
        } => fit_dimensions(original_width, original_height, max_width, max_height),
    };
    let pixels = if (width, height) == (original_width, original_height) {
        decoded.to_rgba8()
    } else {
        let full = decoded.to_rgba8();
        image::imageops::resize(&full, width, height, image::imageops::FilterType::Triangle)
    };
    let mut image = ColorImage::from_rgba_unmultiplied(
        [pixels.width() as usize, pixels.height() as usize],
        pixels.as_raw(),
    );
    image.source_size = Vec2::new(original_width as f32, original_height as f32);
    Ok(image)
}

type Decode = Poll<Result<Arc<ColorImage>, String>>;

#[derive(Default)]
struct Images {
    /// Decodes in flight or finished, keyed by URI and target size.
    decoded: HashMap<(String, Target), Decode>,
    /// The one GPU texture kept for a URI: the largest display size asked
    /// for, or the viewer's full-resolution image.
    gpu: HashMap<String, GpuImage>,
}

struct GpuImage {
    target: Target,
    /// The buffer is the file's own resolution, so a larger hint cannot add
    /// pixels. A display-sized texture that already is the whole file counts.
    complete: bool,
    source_size: Vec2,
    options: TextureOptions,
    handle: TextureHandle,
}

impl GpuImage {
    fn covers(&self, target: Target, options: TextureOptions) -> bool {
        if self.options != options {
            return false;
        }
        if self.complete {
            return true;
        }
        match (self.target, target) {
            (Target::Full, _) => true,
            (_, Target::Full) => false,
            (
                Target::Fit { width, height },
                Target::Fit {
                    width: want_width,
                    height: want_height,
                },
            ) => width >= want_width && height >= want_height,
        }
    }

    fn texture(&self) -> egui::load::SizedTexture {
        egui::load::SizedTexture::new(self.handle.id(), self.source_size)
    }
}

struct Decoded {
    cache: Arc<Mutex<Images>>,
}

impl Decoded {
    const ID: &'static str = egui::generate_loader_id!(Decoded);
}

impl ImageLoader for Decoded {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &Context, uri: &str, size_hint: SizeHint) -> egui::load::ImageLoadResult {
        if !claims(uri) {
            return Err(LoadError::NotSupported);
        }
        let target = target_for(uri, size_hint);
        let key = (identity(uri), target);
        if let Some(entry) = lock(&self.cache).decoded.get(&key).cloned() {
            return match entry {
                Poll::Ready(Ok(image)) => Ok(ImagePoll::Ready { image }),
                Poll::Ready(Err(err)) => Err(LoadError::Loading(err)),
                Poll::Pending => Ok(ImagePoll::Pending { size: None }),
            };
        }
        let bytes = match ctx.try_load_bytes(source_uri(uri)) {
            Ok(egui::load::BytesPoll::Ready { bytes, .. }) => bytes,
            Ok(egui::load::BytesPoll::Pending { size }) => {
                return Ok(ImagePoll::Pending { size });
            }
            Err(err) => return Err(err),
        };
        if image::guess_format(&bytes).is_err() {
            return Err(LoadError::NotSupported);
        }
        lock(&self.cache).decoded.insert(key.clone(), Poll::Pending);
        let cache = Arc::clone(&self.cache);
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("zapfast-image".to_owned())
            .spawn(move || {
                let result = decode(bytes.as_ref(), target).map(Arc::new);
                let mut images = lock(&cache);
                if let std::collections::hash_map::Entry::Occupied(mut entry) =
                    images.decoded.entry(key)
                    && entry.get().is_pending()
                {
                    *entry.get_mut() = Poll::Ready(result);
                    drop(images);
                    ctx.request_repaint();
                }
            })
            .expect("spawn image decode");
        Ok(ImagePoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        let uri = identity(uri);
        lock(&self.cache).decoded.retain(|key, _| key.0 != uri);
    }

    fn forget_all(&self) {
        lock(&self.cache).decoded.clear();
    }

    fn byte_size(&self) -> usize {
        lock(&self.cache)
            .decoded
            .values()
            .map(|entry| match entry {
                Poll::Ready(Ok(image)) => image.pixels.len() * std::mem::size_of::<egui::Color32>(),
                Poll::Ready(Err(err)) => err.len(),
                Poll::Pending => 0,
            })
            .sum()
    }

    fn has_pending(&self) -> bool {
        lock(&self.cache)
            .decoded
            .values()
            .any(|entry| entry.is_pending())
    }
}

struct Uploaded {
    cache: Arc<Mutex<Images>>,
}

impl Uploaded {
    const ID: &'static str = egui::generate_loader_id!(Uploaded);

    fn stored(&self, uri: &str, options: TextureOptions) -> Option<egui::load::SizedTexture> {
        let images = lock(&self.cache);
        let hit = images.gpu.get(&identity(uri))?;
        (hit.options == options).then(|| hit.texture())
    }
}

impl TextureLoader for Uploaded {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(
        &self,
        ctx: &Context,
        uri: &str,
        texture_options: TextureOptions,
        size_hint: SizeHint,
    ) -> egui::load::TextureLoadResult {
        if !claims(uri) {
            return Err(LoadError::NotSupported);
        }
        let target = target_for(uri, size_hint);
        let key = identity(uri);
        {
            let images = lock(&self.cache);
            if let Some(hit) = images.gpu.get(&key)
                && hit.covers(target, texture_options)
            {
                return Ok(egui::load::TexturePoll::Ready {
                    texture: hit.texture(),
                });
            }
        }
        match ctx.try_load_image(uri, size_hint) {
            Err(LoadError::NotSupported) => Err(LoadError::NotSupported),
            Err(err) => self
                .stored(uri, texture_options)
                .map_or(Err(err), |texture| {
                    Ok(egui::load::TexturePoll::Ready { texture })
                }),
            Ok(ImagePoll::Pending { size }) => {
                if let Some(texture) = self.stored(uri, texture_options) {
                    Ok(egui::load::TexturePoll::Ready { texture })
                } else {
                    Ok(egui::load::TexturePoll::Pending { size })
                }
            }
            Ok(ImagePoll::Ready { image }) => {
                let complete = image.width() as f32 == image.source_size.x
                    && image.height() as f32 == image.source_size.y;
                let source_size = image.source_size;
                let buffer = [image.width(), image.height()];
                let handle = ctx.load_texture(uri, image, texture_options);
                let texture = egui::load::SizedTexture::new(handle.id(), source_size);
                {
                    let mut images = lock(&self.cache);
                    images.gpu.insert(
                        key.clone(),
                        GpuImage {
                            target,
                            complete,
                            source_size,
                            options: texture_options,
                            handle,
                        },
                    );
                    images.decoded.remove(&(key.clone(), target));
                }
                // The JPEG of a `bytes://` thumbnail is the only copy of those
                // bytes. `include` will not register them again while this
                // cache entry says they already are, so they stay. A file can
                // be read again for a larger decode.
                if !source_uri(uri).starts_with("bytes://") {
                    let source = source_uri(uri);
                    let loaders = ctx.loaders();
                    loaders.include.forget(source);
                    for loader in loaders.bytes.lock().iter() {
                        loader.forget(source);
                    }
                }
                let (width, height) = buffer_dims(buffer);
                note_retained(ctx, &key, width, height);
                Ok(egui::load::TexturePoll::Ready { texture })
            }
        }
    }

    fn forget(&self, uri: &str) {
        lock(&self.cache).gpu.remove(&identity(uri));
    }

    fn forget_all(&self) {
        lock(&self.cache).gpu.clear();
    }

    fn byte_size(&self) -> usize {
        lock(&self.cache)
            .gpu
            .values()
            .map(|image| image.handle.byte_size())
            .sum()
    }
}

fn buffer_dims(size: [usize; 2]) -> (u32, u32) {
    let dim = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    (dim(size[0]), dim(size[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        let ctx = Context::default();
        egui_extras::install_image_loaders(&ctx);
        install(&ctx);
        ctx
    }

    /// Advances the frame counter the way a repaint would.
    fn frame(ctx: &Context) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
    }

    fn registered(ctx: &Context, uri: &str) -> bool {
        ctx.try_load_bytes(uri).is_ok()
    }

    fn uri(index: usize) -> String {
        format!("bytes://test-{index:04}")
    }

    /// Answers in the same frame, so a test can watch a texture being
    /// allocated without waiting on a decoding thread.
    struct ReadyLoader;

    impl ImageLoader for ReadyLoader {
        fn id(&self) -> &str {
            "zapfast::image_cache::tests::ReadyLoader"
        }

        fn load(
            &self,
            _ctx: &Context,
            _uri: &str,
            _size_hint: SizeHint,
        ) -> egui::load::ImageLoadResult {
            Ok(ImagePoll::Ready {
                image: Arc::new(ColorImage::example()),
            })
        }

        fn forget(&self, _uri: &str) {}

        fn forget_all(&self) {}

        fn byte_size(&self) -> usize {
            0
        }
    }

    /// Draws `shown` and then sweeps, the way a frame of the app does.
    /// Touching them here is what a bubble does, so this frame's images are
    /// the ones the sweep is not allowed to drop.
    fn draw(ctx: &Context, shown: &[String]) -> egui::FullOutput {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for uri in shown {
                touch(ui.ctx(), uri);
                ui.add(egui::Image::from_uri(uri.clone()));
            }
            sweep(ui.ctx());
        })
    }

    fn allocated(ctx: &Context) -> usize {
        let manager = ctx.tex_manager();
        manager.read().num_allocated()
    }

    fn kept(ctx: &Context, total: usize) -> usize {
        (0..total)
            .filter(|index| registered(ctx, &uri(*index)))
            .count()
    }

    #[test]
    fn a_forgotten_image_is_released_and_registers_again_when_drawn() {
        let ctx = context();
        include(&ctx, &uri(1), b"image bytes");
        assert!(registered(&ctx, &uri(1)));
        forget(&ctx, &uri(1));
        assert!(
            !registered(&ctx, &uri(1)),
            "forgotten at once, not swept later"
        );
        include(&ctx, &uri(1), b"image bytes");
        assert!(registered(&ctx, &uri(1)), "a later draw registers it again");
    }

    #[test]
    fn images_the_current_frame_drew_are_never_released() {
        let ctx = context();
        set_budget(&ctx, 1);
        let total = 12;
        for index in 0..total {
            include(&ctx, &uri(index), b"image bytes");
            charge(&ctx, &uri(index), 1_000);
        }
        // A frame that shows more than the budget must not drop what it is
        // showing right now.
        sweep(&ctx);
        assert_eq!(kept(&ctx, total), total);
    }

    #[test]
    fn images_that_scroll_away_are_released() {
        let ctx = context();
        let each = 1_000;
        let keep = 10;
        set_budget(&ctx, keep * each);
        let total = 16;
        for index in 0..total {
            include(&ctx, &uri(index), b"image bytes");
            charge(&ctx, &uri(index), each);
        }
        frame(&ctx);
        sweep(&ctx);
        assert_eq!(
            kept(&ctx, total),
            keep,
            "idle images are released until the byte budget holds"
        );
        assert_eq!(retained(&ctx), keep * each);
    }

    #[test]
    fn a_released_image_is_registered_again_when_it_comes_back() {
        let ctx = context();
        set_budget(&ctx, 1_000);
        for index in 0..4 {
            include(&ctx, &uri(index), b"image bytes");
            charge(&ctx, &uri(index), 1_000);
        }
        frame(&ctx);
        sweep(&ctx);
        // The oldest URI lost its bytes. Drawing it again must put them back,
        // because egui has no other way to resolve a `bytes://` image.
        include(&ctx, &uri(0), b"image bytes");
        assert!(registered(&ctx, &uri(0)));
    }

    #[test]
    fn images_that_scroll_away_free_their_texture() {
        let ctx = context();
        ctx.add_image_loader(Arc::new(ReadyLoader));
        let uris: Vec<String> = (0..12).map(uri).collect();
        let sample = ColorImage::example();
        let each = retained_bytes(sample.width() as u32, sample.height() as u32);
        set_budget(&ctx, each * (uris.len() - 8));
        for uri in &uris {
            include(&ctx, uri, b"image bytes");
        }
        let mut showing = draw(&ctx, &uris);
        let shown = allocated(&ctx);
        assert!(
            shown >= uris.len(),
            "a drawn image should own a texture, saw {shown} for {} images",
            uris.len()
        );
        showing.textures_delta.clear();

        // Nothing on screen: the images past the budget go, and the frame that
        // released them is the one that tells the painter to delete them.
        let mut empty = draw(&ctx, &[]);
        assert_eq!(
            empty.textures_delta.free.len(),
            8,
            "the swept textures should be freed by the painter"
        );
        assert_eq!(allocated(&ctx), shown - 8);
        empty.textures_delta.clear();
    }

    /// 512×512 RGBA is 1_048_576 bytes. The budget counts that buffer and an
    /// equal GPU texture, 2_097_152 bytes an image. The previous cap kept 96
    /// of them (201_326_592 bytes). [`BYTE_BUDGET`] is 50_331_648 and holds 24.
    #[test]
    fn synthetic_images_stay_within_the_byte_budget() {
        const SIDE: u32 = 512;
        const EACH: usize = retained_bytes(SIDE, SIDE);
        const PREVIOUS_CAP: usize = 96;
        const FILLED: usize = 40;
        assert_eq!(EACH, 2_097_152);
        assert_eq!(PREVIOUS_CAP * EACH, 201_326_592);
        assert_eq!(BYTE_BUDGET / EACH, 24);
        assert_eq!(24 * EACH, BYTE_BUDGET);

        let ctx = context();
        for index in 0..FILLED {
            include(&ctx, &uri(index), b"synthetic");
            charge(&ctx, &uri(index), EACH);
        }
        frame(&ctx);
        sweep(&ctx);
        let kept = kept(&ctx, FILLED);
        let bytes = retained(&ctx);
        assert_eq!(kept, BYTE_BUDGET / EACH);
        assert_eq!(bytes, BYTE_BUDGET);
        assert!(bytes < PREVIOUS_CAP * EACH);
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(width, height, image::Rgba([20, 40, 60, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode png");
        bytes.into_inner()
    }

    /// Waits until `uri` is uploaded at `expect` pixels. A larger hint keeps
    /// showing the previous texture while its decode runs, so a ready texture
    /// of the wrong size is not the answer yet.
    fn load_texture(
        ctx: &Context,
        uri: &str,
        hint: SizeHint,
        expect: [usize; 2],
    ) -> (Vec2, [usize; 2]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match ctx.try_load_texture(uri, TextureOptions::LINEAR, hint) {
                Ok(egui::load::TexturePoll::Ready { texture }) => {
                    let gpu = ctx
                        .tex_manager()
                        .read()
                        .meta(texture.id)
                        .expect("texture allocated")
                        .size;
                    if gpu == expect {
                        return (texture.size, gpu);
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "{uri} stayed at {gpu:?}, wanted {expect:?}"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Ok(egui::load::TexturePoll::Pending { .. }) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "{uri} did not finish decoding"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(err) => panic!("{uri} failed to load: {err}"),
            }
        }
    }

    #[test]
    fn display_decodes_fit_the_hint_and_keep_the_file_size() {
        let ctx = context();
        let bytes = png(320, 160);
        include(&ctx, "bytes://photo", &bytes);
        let hint = SizeHint::Size {
            width: 80,
            height: 40,
            maintain_aspect_ratio: true,
        };
        let (source, gpu) = load_texture(&ctx, "bytes://photo", hint, [128, 64]);
        // 80×40 quantizes up to a 128×64 box. The 320×160 file fits that box
        // at 128×64, which is the buffer the GPU holds.
        assert_eq!(
            source,
            Vec2::new(320.0, 160.0),
            "layout still sees the file"
        );
        assert_eq!(gpu, [128, 64]);
        assert_eq!(retained(&ctx), retained_bytes(128, 64));

        // The same box at twice the scale, which is what a HiDPI hint is:
        // egui multiplies points by pixels_per_point before we see them.
        let sharp = SizeHint::Size {
            width: 160,
            height: 80,
            maintain_aspect_ratio: true,
        };
        let (source, gpu) = load_texture(&ctx, "bytes://photo", sharp, [192, 96]);
        assert_eq!(source, Vec2::new(320.0, 160.0));
        assert_eq!(gpu, [192, 96]);
        assert_eq!(retained(&ctx), retained_bytes(192, 96));
    }

    fn full_hint() -> SizeHint {
        SizeHint::Scale(egui::emath::OrderedFloat(1.0))
    }

    #[test]
    fn the_viewer_keeps_every_pixel_until_it_is_released() {
        let ctx = context();
        let bytes = png(320, 160);
        let display = "bytes://photo";
        include(&ctx, display, &bytes);
        let viewer = format!("{FULL_PREFIX}{display}");
        let display_hint = SizeHint::Size {
            width: 64,
            height: 64,
            maintain_aspect_ratio: true,
        };
        // The viewer reads the same bytes through the unprefixed URI.
        let (_, display_gpu) = load_texture(&ctx, display, display_hint, [64, 32]);
        assert_eq!(display_gpu, [64, 32]);
        let (source, gpu) = load_texture(&ctx, &viewer, full_hint(), [320, 160]);
        assert_eq!(source, Vec2::new(320.0, 160.0));
        assert_eq!(gpu, [320, 160], "the viewer uploads the file's own pixels");
        assert_eq!(
            retained(&ctx),
            retained_bytes(64, 32) + retained_bytes(320, 160)
        );

        ctx.data_mut(|data| data.insert_temp(viewer_id(), viewer.clone()));
        release_viewer(&ctx);
        assert_eq!(retained(&ctx), retained_bytes(64, 32));
        let (_, display_again) = load_texture(&ctx, display, display_hint, [64, 32]);
        assert_eq!(
            display_again,
            [64, 32],
            "releasing the viewer leaves the inline texture"
        );
        // The full-resolution URI is forgotten: asking again has to decode.
        assert!(
            matches!(
                ctx.try_load_texture(&viewer, TextureOptions::LINEAR, full_hint()),
                Ok(egui::load::TexturePoll::Pending { .. })
            ),
            "releasing the viewer drops its texture and its decode"
        );
    }

    #[test]
    fn a_webp_frame_suffix_uses_the_file_the_interface_registered() {
        let ctx = context();
        let bytes = png(80, 40);
        include(&ctx, "bytes://photo.webp", &bytes);
        let hint = SizeHint::Size {
            width: 80,
            height: 40,
            maintain_aspect_ratio: true,
        };
        // egui asks for still WebP as `uri#0`. The bytes live under `uri`.
        let (source, gpu) = load_texture(&ctx, "bytes://photo.webp#0", hint, [80, 40]);
        assert_eq!(source, Vec2::new(80.0, 40.0));
        assert_eq!(gpu, [80, 40]);
        assert_eq!(retained(&ctx), retained_bytes(80, 40));
        let id = match ctx.try_load_texture("bytes://photo.webp#0", TextureOptions::LINEAR, hint) {
            Ok(egui::load::TexturePoll::Ready { texture }) => texture.id,
            _ => panic!("the framed URI should still be ready"),
        };
        forget(&ctx, "bytes://photo.webp");
        assert!(
            ctx.tex_manager().read().meta(id).is_none(),
            "forgetting the plain URI drops the texture requested with #0"
        );
    }

    #[test]
    fn a_small_image_is_not_enlarged_to_the_hint() {
        let (width, height) = fit_dimensions(40, 20, 128, 128);
        assert_eq!((width, height), (40, 20));
    }

    #[test]
    fn fit_dimensions_preserve_aspect_inside_the_box() {
        assert_eq!(fit_dimensions(320, 160, 128, 64), (128, 64));
        assert_eq!(fit_dimensions(320, 160, 192, 128), (192, 96));
        assert_eq!(quantize(80), 128);
        assert_eq!(quantize(160), 192);
    }
}
