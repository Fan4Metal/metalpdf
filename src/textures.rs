//! Page textures uploaded to OpenGL directly with glow, not through egui's
//! texture manager: egui_glow uploads RGBA with `glTexImage2D` and then
//! calls `glGenerateMipmap`, which took 70–150 ms of the UI thread for a
//! 15-megapixel photo in qview (the driver converts the pixels and builds
//! the mipmaps itself). Here the engine thread delivers BGRx pixels in the
//! driver's native order and every mip level (`pixels::Pixels`), so the UI
//! thread only hands them to the driver (3–10 ms for a page once the
//! driver's memory pool is warm). Taken from qview's `texture.rs`.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

use glow::HasContext;

use crate::pixels::Pixels;

/// A texture of the GL context `gl`, registered with egui as `id`. Deleted
/// after it is dropped, by [`delete_dropped`] on the UI thread while the
/// context lives (see `App::on_exit`).
pub struct Texture {
    gl: Arc<glow::Context>,
    native: glow::Texture,
    id: egui::TextureId,
    pub width: u32,
    pub height: u32,
    /// Bytes of every level, for the cache's budget.
    pub bytes: usize,
}

impl Texture {
    /// Upload every level of `pixels` and register the texture with egui
    /// through `register` (`eframe::Frame::register_native_glow_texture`).
    pub fn new(
        gl: Arc<glow::Context>,
        pixels: &Pixels,
        register: impl FnOnce(glow::Texture) -> egui::TextureId,
    ) -> Result<Self, String> {
        let started = Instant::now();
        let levels = pixels.levels.len() as u32;
        let native = unsafe {
            let native = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(native));
            let param = |name: u32, value: u32| gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
            param(glow::TEXTURE_MIN_FILTER, if levels > 1 { glow::LINEAR_MIPMAP_LINEAR } else { glow::LINEAR });
            param(glow::TEXTURE_MAG_FILTER, glow::LINEAR);
            param(glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE);
            param(glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE);
            param(glow::TEXTURE_MAX_LEVEL, levels - 1);
            // The fourth byte of BGRx is whatever PDFium left there: read
            // the texture as opaque.
            param(glow::TEXTURE_SWIZZLE_A, glow::ONE);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            for (level, data) in pixels.levels.iter().enumerate() {
                let (w, h) = pixels.level_size(level);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    level as i32,
                    glow::RGBA8 as i32,
                    w as i32,
                    h as i32,
                    0,
                    glow::BGRA,
                    glow::UNSIGNED_INT_8_8_8_8_REV,
                    glow::PixelUnpackData::Slice(Some(data)),
                );
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            // glGetError waits for the driver's worker thread to finish the
            // upload (20–60 ms): only in debug builds.
            let error = if cfg!(debug_assertions) { gl.get_error() } else { glow::NO_ERROR };
            if error != glow::NO_ERROR {
                gl.delete_texture(native);
                return Err(format!("OpenGL error {error:#x} while uploading the texture"));
            }
            native
        };
        log::debug!(
            "uploaded {}x{} ({} levels) in {:.1} ms",
            pixels.width,
            pixels.height,
            levels,
            started.elapsed().as_secs_f64() * 1e3
        );
        let id = register(native);
        Ok(Self { gl, native, id, width: pixels.width, height: pixels.height, bytes: pixels.bytes() })
    }

    pub fn id(&self) -> egui::TextureId {
        self.id
    }
}

thread_local! {
    /// Textures dropped since the last [`delete_dropped`]. A texture
    /// dropped while a frame is built may still be in that frame's shapes:
    /// deleted at once, the frame showed whatever the driver had put in
    /// its memory.
    static DROPPED: RefCell<Vec<(Arc<glow::Context>, glow::Texture)>> = const { RefCell::new(Vec::new()) };
}

/// Delete the textures dropped since the last call: at the start of a
/// frame, when the previous one has been painted, and on exit.
pub fn delete_dropped() {
    for (gl, native) in DROPPED.take() {
        unsafe { gl.delete_texture(native) };
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        DROPPED.with_borrow_mut(|d| d.push((self.gl.clone(), self.native)));
    }
}
