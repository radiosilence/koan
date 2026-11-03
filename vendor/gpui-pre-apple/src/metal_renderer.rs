use crate::metal_atlas::MetalAtlas;
use anyhow::{Context as _, Result};
use block2::RcBlock;
use core_graphics::geometry::CGSize;
use gpui::{
    AtlasTextureId, Background, Bounds, ContentMask, DevicePixels, PaintSurface, Path, Point,
    PrimitiveBatch, ScaledPixels, Scene, Size, point, size,
};
#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
use image::RgbaImage;
use objc2::runtime::AnyObject;

use core_graphics::color_space::{CGColorSpace, kCGColorSpaceDisplayP3};
use foreign_types::{ForeignType, ForeignTypeRef};
use metal::{
    CAMetalLayer, CommandQueue, MTLGPUFamily, MTLPixelFormat, MTLResourceOptions, NSRange,
    NSUInteger,
};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBufferGetHeight,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferGetWidthOfPlane, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVReturnSuccess,
};
use parking_lot::Mutex;

use std::{cell::Cell, ffi::c_void, mem, mem::MaybeUninit, ops::Range, ptr, slice, sync::Arc};

// Exported to metal
pub(crate) type PointF = gpui::Point<f32>;

#[cfg(not(feature = "runtime_shaders"))]
const SHADERS_METALLIB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shaders.metallib"));
#[cfg(feature = "runtime_shaders")]
const SHADERS_SOURCE_FILE: &str = include_str!(concat!(env!("OUT_DIR"), "/stitched_shaders.metal"));
// Use 4x MSAA, all devices support it.
// https://developer.apple.com/documentation/metal/mtldevice/1433355-supportstexturesamplecount
const PATH_SAMPLE_COUNT: u32 = 4;
/// Metal requires the offset a buffer is bound at to be 256-byte aligned.
const INSTANCE_BUFFER_ALIGNMENT: usize = 256;
const MAX_INSTANCE_BUFFER_SIZE: usize = 256 * 1024 * 1024;

pub type Context = Arc<Mutex<InstanceBufferPool>>;
pub type Renderer = MetalRenderer;

pub unsafe fn new_renderer(
    context: self::Context,
    _native_window: *mut c_void,
    _native_view: *mut c_void,
    _bounds: gpui::Size<f32>,
    transparent: bool,
    use_display_p3: bool,
) -> Renderer {
    MetalRenderer::new(context, transparent, use_display_p3)
}

pub struct InstanceBufferPool {
    buffer_size: usize,
    buffers: Vec<metal::Buffer>,
}

impl Default for InstanceBufferPool {
    fn default() -> Self {
        Self {
            buffer_size: 2 * 1024 * 1024,
            buffers: Vec::new(),
        }
    }
}

pub(crate) struct InstanceBuffer {
    metal_buffer: metal::Buffer,
    size: usize,
}

impl InstanceBufferPool {
    pub(crate) fn reset(&mut self, buffer_size: usize) {
        self.buffer_size = buffer_size;
        self.buffers.clear();
    }

    pub(crate) fn acquire(
        &mut self,
        device: &metal::Device,
        unified_memory: bool,
    ) -> InstanceBuffer {
        let buffer = self.buffers.pop().unwrap_or_else(|| {
            let options = if unified_memory {
                MTLResourceOptions::StorageModeShared
                    // Buffers are write only which can benefit from the combined cache
                    // https://developer.apple.com/documentation/metal/mtlresourceoptions/cpucachemodewritecombined
                    | MTLResourceOptions::CPUCacheModeWriteCombined
            } else {
                MTLResourceOptions::StorageModeManaged
            };

            device.new_buffer(self.buffer_size as u64, options)
        });
        InstanceBuffer {
            metal_buffer: buffer,
            size: self.buffer_size,
        }
    }

    pub(crate) fn release(&mut self, buffer: InstanceBuffer) {
        if buffer.size == self.buffer_size {
            self.buffers.push(buffer.metal_buffer)
        }
    }
}

pub struct MetalRenderer {
    device: metal::Device,
    layer: Option<metal::MetalLayer>,
    is_apple_gpu: bool,
    is_unified_memory: bool,
    presents_with_transaction: bool,
    /// For headless rendering, tracks whether output should be opaque
    opaque: bool,
    command_queue: CommandQueue,
    paths_rasterization_pipeline_state: metal::RenderPipelineState,
    path_sprites_pipeline_state: metal::RenderPipelineState,
    shadows_pipeline_state: metal::RenderPipelineState,
    quads_pipeline_state: metal::RenderPipelineState,
    underlines_pipeline_state: metal::RenderPipelineState,
    monochrome_sprites_pipeline_state: metal::RenderPipelineState,
    polychrome_sprites_pipeline_state: metal::RenderPipelineState,
    surfaces_pipeline_state: metal::RenderPipelineState,
    // KOAN: each shader program's pipeline, compiled the first time it is
    // drawn; `None` for one that would not compile, which is not tried again
    // (#1641).
    shader_pipelines: std::cell::RefCell<
        std::collections::HashMap<&'static str, Option<metal::RenderPipelineState>>,
    >,
    // KOAN: the last frame drawn as the layers its shaders are drawn
    // between, which each present composes with the shaders (#1641).
    shader_layers: ShaderLayerTextures,
    // KOAN: each layer group's texture, and the frame they were drawn for
    // (#1859).
    group_textures: Vec<Option<metal::Texture>>,
    group_generation: u64,
    // KOAN: whether each group moved when last drawn, and the colour slot
    // table it was drawn against (#1878).
    group_moving: Vec<bool>,
    group_slot_generation: u64,
    compose_pipelines: Option<ComposePipelines>,
    /// What a shader with no image is given to sample.
    blank_texture: Option<metal::Texture>,
    unit_vertices: metal::Buffer,
    #[allow(clippy::arc_with_non_send_sync)]
    instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    sprite_atlas: Arc<MetalAtlas>,
    core_video_texture_cache: CFRetained<CVMetalTextureCache>,
    path_intermediate_texture: Option<metal::Texture>,
    path_intermediate_msaa_texture: Option<metal::Texture>,
    path_sample_count: u32,
    /// Offscreen render target reused across `render_scene` calls when
    /// rendering headlessly without reading pixels back.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    headless_render_target: Option<metal::Texture>,
}

#[repr(C)]
pub struct PathRasterizationVertex {
    pub xy_position: Point<ScaledPixels>,
    pub st_position: Point<f32>,
    pub color: Background,
    pub bounds: Bounds<ScaledPixels>,
}

impl MetalRenderer {
    /// Creates a new MetalRenderer with a CAMetalLayer for window-based rendering.
    pub fn new(
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
        transparent: bool,
        use_display_p3: bool,
    ) -> Self {
        let device = Self::create_device();
        let layer = metal::MetalLayer::new();
        Self::configure_layer(&layer, &device, transparent, use_display_p3);
        Self::new_internal(device, Some(layer), !transparent, instance_buffer_pool)
    }

    /// Creates a renderer for a CAMetalLayer owned by a platform view, such as
    /// the backing layer UIKit creates for a view whose `layerClass` is
    /// `CAMetalLayer`. The renderer retains the layer.
    pub fn from_layer(
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
        layer: &objc2_quartz_core::CAMetalLayer,
        transparent: bool,
    ) -> Self {
        let device = Self::create_device();
        // Both types bind the same Objective-C class, so this only changes which
        // Rust wrapper views the live layer. `to_owned` retains it.
        let layer = unsafe {
            metal::MetalLayerRef::from_ptr(ptr::from_ref(layer).cast_mut().cast::<CAMetalLayer>())
        }
        .to_owned();
        Self::configure_layer(&layer, &device, transparent, false);
        Self::new_internal(device, Some(layer), !transparent, instance_buffer_pool)
    }

    fn configure_layer(
        layer: &metal::MetalLayerRef,
        device: &metal::DeviceRef,
        transparent: bool,
        use_display_p3: bool,
    ) {
        layer.set_device(device);
        layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        // metal-rs doesn't bind these setters, so view the same object through
        // objc2's typed CAMetalLayer binding.
        let objc2_layer: &objc2_quartz_core::CAMetalLayer = unsafe { &*layer.as_ptr().cast() };
        if use_display_p3 {
            // SAFETY: The constant is extern, from Core Graphics
            let display_p3_name = unsafe { kCGColorSpaceDisplayP3 };
            if let Some(display_p3_color_space) = CGColorSpace::create_with_name(display_p3_name) {
                // SAFETY: `layer` is a CAMetalLayer and the color space remains valid for the call.
                // 'metal-rs' does not expose CAMetalLayer's typed `colorspace` setter.
                // Could be replaced with a native binding via objc2-* in future
                unsafe {
                    let _: () = objc2::msg_send![
                        objc2_layer,
                        setColorspace: display_p3_color_space.as_ptr().cast::<c_void>()
                    ];
                }
            } else {
                log::error!("unable to create the Display P3 color space");
            }
        }
        // Support direct-to-display rendering if the window is not transparent
        // https://developer.apple.com/documentation/metal/managing-your-game-window-for-metal-in-macos
        layer.set_opaque(!transparent);
        layer.set_maximum_drawable_count(3);
        // Allow texture reading for visual tests and UI automation screenshots
        // (captures without ScreenCaptureKit). Only debug builds pay the
        // presentation cost, even when `test-support` is compiled in.
        #[cfg(all(feature = "test-support", debug_assertions))]
        layer.set_framebuffer_only(false);
        objc2_layer.setAllowsNextDrawableTimeout(false);
        objc2_layer.setNeedsDisplayOnBoundsChange(true);
        // UIKit sizes a view's backing layer itself; only AppKit-hosted
        // layers need to track their superlayer's bounds.
        #[cfg(target_os = "macos")]
        objc2_layer.setAutoresizingMask(
            objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
                | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
        );
    }

    /// Creates a new headless MetalRenderer for offscreen rendering without a window.
    ///
    /// This renderer can render scenes to images without requiring a CAMetalLayer,
    /// window, or AppKit. Use `render_scene_to_image()` to render scenes.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn new_headless(instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>) -> Self {
        let device = Self::create_device();
        Self::new_internal(device, None, true, instance_buffer_pool)
    }

    #[cfg(target_os = "macos")]
    fn create_device() -> metal::Device {
        // Prefer low‐power integrated GPUs on Intel Mac. On Apple
        // Silicon, there is only ever one GPU, so this is equivalent to
        // `metal::Device::system_default()`.
        if let Some(d) = metal::Device::all()
            .into_iter()
            .min_by_key(|d| (d.is_removable(), !d.is_low_power()))
        {
            d
        } else {
            // For some reason `all()` can return an empty list, see https://github.com/zed-industries/zed/issues/37689
            // In that case, we fall back to the system default device.
            log::error!(
                "Unable to enumerate Metal devices; attempting to use system default device"
            );
            Self::system_default_device()
        }
    }

    #[cfg(target_os = "ios")]
    fn create_device() -> metal::Device {
        Self::system_default_device()
    }

    fn system_default_device() -> metal::Device {
        metal::Device::system_default().unwrap_or_else(|| {
            log::error!("unable to access a compatible graphics device");
            std::process::exit(1);
        })
    }

    fn new_internal(
        device: metal::Device,
        layer: Option<metal::MetalLayer>,
        opaque: bool,
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    ) -> Self {
        #[cfg(feature = "runtime_shaders")]
        let library = device
            .new_library_with_source(&SHADERS_SOURCE_FILE, &metal::CompileOptions::new())
            .expect("error building metal library");
        #[cfg(not(feature = "runtime_shaders"))]
        let library = device
            .new_library_with_data(SHADERS_METALLIB)
            .expect("error building metal library");

        fn to_float2_bits(point: PointF) -> u64 {
            let mut output = point.y.to_bits() as u64;
            output <<= 32;
            output |= point.x.to_bits() as u64;
            output
        }

        // Shared memory can be used only if CPU and GPU share the same memory space.
        // https://developer.apple.com/documentation/metal/setting-resource-storage-modes
        // iOS does not support managed resources. Its simulator may report a
        // non-unified host GPU even though resources must still use shared storage.
        let is_unified_memory = cfg!(target_os = "ios") || device.has_unified_memory();
        // Apple GPU families support memoryless textures, which can significantly reduce
        // memory usage by keeping render targets in on-chip tile memory instead of
        // allocating backing store in system memory.
        // https://developer.apple.com/documentation/metal/mtlgpufamily
        let is_apple_gpu = device.supports_family(MTLGPUFamily::Apple1);

        let unit_vertices = [
            to_float2_bits(point(0., 0.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(1., 1.)),
        ];
        let unit_vertices = device.new_buffer_with_data(
            unit_vertices.as_ptr() as *const c_void,
            mem::size_of_val(&unit_vertices) as u64,
            if is_unified_memory {
                MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined
            } else {
                MTLResourceOptions::StorageModeManaged
            },
        );

        let paths_rasterization_pipeline_state = build_path_rasterization_pipeline_state(
            &device,
            &library,
            "paths_rasterization",
            "path_rasterization_vertex",
            "path_rasterization_fragment",
            MTLPixelFormat::BGRA8Unorm,
            PATH_SAMPLE_COUNT,
        );
        let path_sprites_pipeline_state = build_path_sprite_pipeline_state(
            &device,
            &library,
            "path_sprites",
            "path_sprite_vertex",
            "path_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let shadows_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "shadows",
            "shadow_vertex",
            "shadow_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let quads_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "quads",
            "quad_vertex",
            "quad_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let underlines_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "underlines",
            "underline_vertex",
            "underline_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let monochrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "monochrome_sprites",
            "monochrome_sprite_vertex",
            "monochrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let polychrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "polychrome_sprites",
            "polychrome_sprite_vertex",
            "polychrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let surfaces_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "surfaces",
            "surface_vertex",
            "surface_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );

        let command_queue = device.new_command_queue();
        let supports_shared_storage = cfg!(target_os = "ios") || is_apple_gpu;
        let sprite_atlas = Arc::new(MetalAtlas::new(device.clone(), supports_shared_storage));
        // metal and objc2-metal expose the same Objective-C device; the borrowed
        // protocol object remains valid while `device` owns it.
        let core_video_texture_cache = unsafe {
            let mut cache = ptr::null_mut();
            let status = CVMetalTextureCache::create(
                None,
                None,
                &*device
                    .as_ptr()
                    .cast::<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>>(),
                None,
                ptr::NonNull::from(&mut cache),
            );
            assert_eq!(
                status, kCVReturnSuccess,
                "failed to create CoreVideo Metal texture cache"
            );
            CFRetained::from_raw(ptr::NonNull::new(cache).expect("CoreVideo returned a null cache"))
        };

        Self {
            device,
            layer,
            presents_with_transaction: false,
            is_apple_gpu,
            is_unified_memory,
            opaque,
            command_queue,
            paths_rasterization_pipeline_state,
            path_sprites_pipeline_state,
            shadows_pipeline_state,
            quads_pipeline_state,
            shader_pipelines: Default::default(), // KOAN: #1641.
            shader_layers: Default::default(),    // KOAN: #1641.
            group_textures: Vec::new(),           // KOAN: #1859.
            group_generation: 0,
            group_moving: Vec::new(),
            group_slot_generation: 0,
            compose_pipelines: None, // KOAN: #1641.
            blank_texture: None,     // KOAN: #1641.
            underlines_pipeline_state,
            monochrome_sprites_pipeline_state,
            polychrome_sprites_pipeline_state,
            surfaces_pipeline_state,
            unit_vertices,
            instance_buffer_pool,
            sprite_atlas,
            core_video_texture_cache,
            path_intermediate_texture: None,
            path_intermediate_msaa_texture: None,
            path_sample_count: PATH_SAMPLE_COUNT,
            #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
            headless_render_target: None,
        }
    }

    pub fn layer(&self) -> Option<&metal::MetalLayerRef> {
        self.layer.as_ref().map(|l| l.as_ref())
    }

    pub fn layer_ptr(&self) -> *mut CAMetalLayer {
        self.layer
            .as_ref()
            .map(|l| l.as_ptr())
            .unwrap_or(ptr::null_mut())
    }

    pub fn sprite_atlas(&self) -> &Arc<MetalAtlas> {
        &self.sprite_atlas
    }

    pub fn set_presents_with_transaction(&mut self, presents_with_transaction: bool) {
        self.presents_with_transaction = presents_with_transaction;
        if let Some(layer) = &self.layer {
            layer.set_presents_with_transaction(presents_with_transaction);
        }
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        if let Some(layer) = &self.layer {
            layer.set_drawable_size(CGSize::new(size.width.0 as f64, size.height.0 as f64));
        }
        self.update_path_intermediate_textures(size);
    }

    fn update_path_intermediate_textures(&mut self, size: Size<DevicePixels>) {
        // We are uncertain when this happens, but sometimes size can be 0 here. Most likely before
        // the layout pass on window creation. Zero-sized texture creation causes SIGABRT.
        // https://github.com/zed-industries/zed/issues/36229
        if size.width.0 <= 0 || size.height.0 <= 0 {
            self.path_intermediate_texture = None;
            self.path_intermediate_msaa_texture = None;
            return;
        }

        let texture_descriptor = metal::TextureDescriptor::new();
        texture_descriptor.set_width(size.width.0 as u64);
        texture_descriptor.set_height(size.height.0 as u64);
        texture_descriptor.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
        texture_descriptor.set_storage_mode(metal::MTLStorageMode::Private);
        texture_descriptor
            .set_usage(metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead);
        self.path_intermediate_texture = Some(self.device.new_texture(&texture_descriptor));

        if self.path_sample_count > 1 {
            // https://developer.apple.com/documentation/metal/choosing-a-resource-storage-mode-for-apple-gpus
            // Rendering MSAA textures are done in a single pass, so we can use memory-less storage on Apple Silicon
            let storage_mode = if self.is_apple_gpu {
                metal::MTLStorageMode::Memoryless
            } else {
                metal::MTLStorageMode::Private
            };

            let msaa_descriptor = texture_descriptor;
            msaa_descriptor.set_texture_type(metal::MTLTextureType::D2Multisample);
            msaa_descriptor.set_storage_mode(storage_mode);
            msaa_descriptor.set_sample_count(self.path_sample_count as _);
            self.path_intermediate_msaa_texture = Some(self.device.new_texture(&msaa_descriptor));
        } else {
            self.path_intermediate_msaa_texture = None;
        }
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        self.opaque = !transparent;
        if let Some(layer) = &self.layer {
            layer.set_opaque(!transparent);
        }
    }

    pub fn destroy(&self) {
        // nothing to do
    }

    pub fn draw(&mut self, scene: &Scene) {
        let layer = match &self.layer {
            Some(l) => l.clone(),
            None => {
                log::error!(
                    "draw() called on headless renderer - use render_scene_to_image() instead"
                );
                return;
            }
        };
        let viewport_size = layer.drawable_size();
        let viewport_size: Size<DevicePixels> = size(
            (viewport_size.width.ceil() as i32).into(),
            (viewport_size.height.ceil() as i32).into(),
        );
        let drawable = if let Some(drawable) = layer.next_drawable() {
            drawable
        } else {
            log::error!(
                "failed to retrieve next drawable, drawable size: {:?}",
                viewport_size
            );
            return;
        };

        // KOAN: each layer group drawn into its texture, once per frame
        // drawn (#1859).
        if let Err(error) = self.draw_layer_groups(scene, viewport_size) {
            log::error!("failed to draw layer groups: {error:#}");
        }
        // KOAN: a scene with shaders is drawn once as the layers they are
        // drawn between, and each present composes the layers with the
        // shaders, so re-presenting a moving shader costs the shaders and a
        // few full-screen copies, not the scene (#1641).
        let rendered = if scene.shader_layers.is_some() {
            self.compose_frame(scene, drawable.texture(), viewport_size)
        } else {
            self.shader_layers.release();
            self.render_frame(scene, drawable.texture(), viewport_size, false)
        };
        let command_buffer = match rendered {
            Ok(command_buffer) => command_buffer,
            Err(error) => {
                log::error!("failed to render: {error:#}");
                return;
            }
        };

        if self.presents_with_transaction {
            command_buffer.commit();
            command_buffer.wait_until_scheduled();
            drawable.present();
        } else {
            command_buffer.present_drawable(drawable);
            command_buffer.commit();
        }
    }

    fn render_frame(
        &mut self,
        scene: &Scene,
        texture: &metal::TextureRef,
        viewport_size: Size<DevicePixels>,
        transparent: bool, // KOAN: a layer of a scene with shaders (#1641).
    ) -> Result<metal::CommandBuffer> {
        let mut writer = InstanceBufferWriter::new(
            &self.device,
            &self.instance_buffer_pool,
            self.is_unified_memory,
        );
        let instance_bindings = write_instances(scene, &mut writer).with_context(|| {
            format!(
                "scene too large: {} paths, {} shadows, {} quads, {} underlines, {} mono, {} poly, {} surfaces",
                scene.paths.len(),
                scene.shadows.len(),
                scene.quads.len(),
                scene.underlines.len(),
                scene.monochrome_sprites.len(),
                scene.polychrome_sprites.len(),
                scene.surfaces.len(),
            )
        })?;
        let command_buffer = self.draw_primitives_to_texture(
            scene,
            &instance_bindings,
            &mut writer,
            texture,
            viewport_size,
            transparent,
        )?;

        let instance_buffer_pool = self.instance_buffer_pool.clone();
        let instance_buffer = Cell::new(Some(writer.finish()));
        let block = RcBlock::new(move |buffer: ptr::NonNull<AnyObject>| {
            note_gpu_time(buffer); // KOAN: #2034.
            if let Some(instance_buffer) = instance_buffer.take() {
                instance_buffer_pool.lock().release(instance_buffer);
            }
        });
        // SAFETY: Both pointee types are opaque views of the same Objective-C block pointer ABI.
        unsafe {
            command_buffer.add_completed_handler(&*RcBlock::as_ptr(&block).cast());
        }

        Ok(command_buffer)
    }

    /// Renders the scene to a texture and returns the pixel data as an RGBA image.
    /// This does not present the frame to screen - useful for visual testing
    /// where we want to capture what would be rendered without displaying it.
    ///
    /// Note: This requires a layer-backed renderer. For headless rendering,
    /// use `render_scene_to_image()` instead.
    #[cfg(any(test, feature = "test-support"))]
    pub fn render_to_image(&mut self, scene: &Scene) -> Result<RgbaImage> {
        let layer = self
            .layer
            .clone()
            .ok_or_else(|| anyhow::anyhow!("render_to_image requires a layer-backed renderer"))?;
        let viewport_size = layer.drawable_size();
        let viewport_size: Size<DevicePixels> = size(
            (viewport_size.width.ceil() as i32).into(),
            (viewport_size.height.ceil() as i32).into(),
        );
        let drawable = layer
            .next_drawable()
            .ok_or_else(|| anyhow::anyhow!("Failed to get drawable for render_to_image"))?;

        let command_buffer = self.render_frame(scene, drawable.texture(), viewport_size, false)?;

        // Commit and wait for completion without presenting
        command_buffer.commit();
        command_buffer.wait_until_completed();

        read_texture_to_image(drawable.texture())
    }

    /// Renders a scene to an image without requiring a window or CAMetalLayer.
    ///
    /// This is the primary method for headless rendering. It creates an offscreen
    /// texture, renders the scene to it, and returns the pixel data as an RGBA image.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> Result<RgbaImage> {
        if size.width.0 <= 0 || size.height.0 <= 0 {
            anyhow::bail!("Invalid size for render_scene_to_image: {:?}", size);
        }

        // Headless callers do not have a Cocoa event-loop pool to release
        // autoreleased command buffers and render-pass descriptors.
        objc2::rc::autoreleasepool(|_| {
            // Update path intermediate textures for this size
            self.update_path_intermediate_textures(size);
            self.draw_layer_groups(scene, size)?; // KOAN: #1859.

            // Create an offscreen texture as render target
            let texture_descriptor = metal::TextureDescriptor::new();
            texture_descriptor.set_width(size.width.0 as u64);
            texture_descriptor.set_height(size.height.0 as u64);
            texture_descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            texture_descriptor.set_usage(
                metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead,
            );
            // Like the atlas, only Apple GPUs can create shared textures on macOS;
            // Intel Macs cannot, even with unified memory. iOS has no managed storage.
            let uses_shared_storage = cfg!(target_os = "ios") || self.is_apple_gpu;
            texture_descriptor.set_storage_mode(if uses_shared_storage {
                metal::MTLStorageMode::Shared
            } else {
                metal::MTLStorageMode::Managed
            });
            let target_texture = self.device.new_texture(&texture_descriptor);

            let command_buffer = self.render_frame(scene, &target_texture, size, false)?;

            // Managed textures require an explicit blit synchronize before the CPU
            // can read back the rendered data. Without this, get_bytes returns
            // stale zeros.
            if !uses_shared_storage {
                let blit = command_buffer.new_blit_command_encoder();
                blit.synchronize_resource(&target_texture);
                blit.end_encoding();
            }

            // Commit and wait for completion
            command_buffer.commit();
            command_buffer.wait_until_completed();

            read_texture_to_image(&target_texture)
        })
    }

    // KOAN: (#1641) renders a scene as a window with shaders presents it,
    // the base first and the shaders composed over it, `presents` times,
    // for checking against `render_scene_to_image`.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn compose_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
        presents: usize,
    ) -> Result<RgbaImage> {
        objc2::rc::autoreleasepool(|_| {
            self.update_path_intermediate_textures(size);
            self.draw_layer_groups(scene, size)?; // KOAN: #1859.
            let texture_descriptor = metal::TextureDescriptor::new();
            texture_descriptor.set_width(size.width.0 as u64);
            texture_descriptor.set_height(size.height.0 as u64);
            texture_descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            texture_descriptor.set_usage(
                metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead,
            );
            let uses_shared_storage = cfg!(target_os = "ios") || self.is_apple_gpu;
            texture_descriptor.set_storage_mode(if uses_shared_storage {
                metal::MTLStorageMode::Shared
            } else {
                metal::MTLStorageMode::Managed
            });
            let target_texture = self.device.new_texture(&texture_descriptor);
            let mut last = None;
            for _ in 0..presents.max(1) {
                let command_buffer = self.compose_frame(scene, &target_texture, size)?;
                command_buffer.commit();
                last = Some(command_buffer);
            }
            let command_buffer = last.context("no present")?;
            command_buffer.wait_until_completed();
            if !uses_shared_storage {
                let command_buffer = self.command_queue.new_command_buffer();
                let blit = command_buffer.new_blit_command_encoder();
                blit.synchronize_resource(&target_texture);
                blit.end_encoding();
                command_buffer.commit();
                command_buffer.wait_until_completed();
            }
            read_texture_to_image(&target_texture)
        })
    }

    /// Renders a scene to a reused offscreen texture without reading pixels
    /// back or blocking on GPU completion.
    ///
    /// This mirrors the CPU cost of presenting a frame to a window (scene
    /// encoding, instance buffer writes, command submission) and is used by
    /// headless benchmark rendering, where the produced pixels are never
    /// inspected.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> Result<()> {
        if size.width.0 <= 0 || size.height.0 <= 0 {
            anyhow::bail!("Invalid size for render_scene: {:?}", size);
        }

        objc2::rc::autoreleasepool(|_| {
            self.update_path_intermediate_textures(size);

            let needs_new_target = self.headless_render_target.as_ref().is_none_or(|texture| {
                texture.width() != size.width.0 as u64 || texture.height() != size.height.0 as u64
            });
            if needs_new_target {
                let texture_descriptor = metal::TextureDescriptor::new();
                texture_descriptor.set_width(size.width.0 as u64);
                texture_descriptor.set_height(size.height.0 as u64);
                texture_descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                texture_descriptor.set_usage(
                    metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead,
                );
                texture_descriptor.set_storage_mode(metal::MTLStorageMode::Private);
                self.headless_render_target = Some(self.device.new_texture(&texture_descriptor));
            }
            let target_texture = self
                .headless_render_target
                .clone()
                .expect("just ensured the render target exists");

            let command_buffer = self.render_frame(scene, &target_texture, size, false)?;

            // Commit without waiting, mirroring presentation to a real window where
            // the CPU doesn't block on the GPU.
            command_buffer.commit();
            Ok(())
        })
    }

    fn draw_primitives_to_texture(
        &mut self,
        scene: &Scene,
        instance_bindings: &InstanceBindings,
        writer: &mut InstanceBufferWriter,
        texture: &metal::TextureRef,
        viewport_size: Size<DevicePixels>,
        transparent: bool, // KOAN: #1641.
    ) -> Result<metal::CommandBuffer> {
        let command_queue = self.command_queue.clone();
        let command_buffer = command_queue.new_command_buffer();
        let alpha = if self.opaque && !transparent { 1. } else { 0. };

        let mut command_encoder = new_command_encoder_for_texture(
            command_buffer,
            texture,
            viewport_size,
            Some(metal::MTLClearColor::new(0., 0., 0., alpha)),
        );

        for batch in scene.batches() {
            match batch {
                PrimitiveBatch::Shadows(range) => {
                    self.draw_shadows(range, instance_bindings, viewport_size, command_encoder)
                }
                PrimitiveBatch::Quads(range) => {
                    self.draw_quads(range, instance_bindings, viewport_size, command_encoder)
                }
                PrimitiveBatch::Paths(range) => {
                    let paths = &scene.paths[range];
                    command_encoder.end_encoding();

                    let did_draw = self.draw_paths_to_intermediate(
                        paths,
                        writer,
                        &instance_bindings.color_slots, // KOAN: #1859.
                        viewport_size,
                        command_buffer,
                    )?;

                    command_encoder = new_command_encoder_for_texture(
                        command_buffer,
                        texture,
                        viewport_size,
                        None,
                    );

                    if did_draw {
                        if let Err(error) = self.draw_paths_from_intermediate(
                            paths,
                            writer,
                            viewport_size,
                            command_encoder,
                        ) {
                            command_encoder.end_encoding();
                            return Err(error);
                        }
                    }
                }
                PrimitiveBatch::Underlines(range) => {
                    self.draw_underlines(range, instance_bindings, viewport_size, command_encoder)
                }
                PrimitiveBatch::MonochromeSprites { texture_id, range } => self
                    .draw_monochrome_sprites(
                        texture_id,
                        range,
                        instance_bindings,
                        viewport_size,
                        command_encoder,
                    ),
                PrimitiveBatch::PolychromeSprites { texture_id, range } => self
                    .draw_polychrome_sprites(
                        texture_id,
                        range,
                        instance_bindings,
                        viewport_size,
                        command_encoder,
                    ),
                // KOAN: shader quads (#1641).
                PrimitiveBatch::Shaders {
                    program,
                    texture,
                    group,
                    range,
                } => self.draw_shaders(
                    program,
                    texture,
                    group,
                    range,
                    instance_bindings,
                    viewport_size,
                    command_encoder,
                ),
                PrimitiveBatch::Surfaces(range) => self.draw_surfaces(
                    &scene.surfaces[range.clone()],
                    range.start,
                    instance_bindings,
                    viewport_size,
                    command_encoder,
                ),
                PrimitiveBatch::SubpixelSprites { .. } => unreachable!(),
            }
        }

        command_encoder.end_encoding();

        Ok(command_buffer.to_owned())
    }

    fn draw_paths_to_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        writer: &mut InstanceBufferWriter,
        color_slots: &InstanceBinding, // KOAN: #1859.
        viewport_size: Size<DevicePixels>,
        command_buffer: &metal::CommandBufferRef,
    ) -> Result<bool> {
        if paths.is_empty() {
            return Ok(false);
        }
        let intermediate_texture = self
            .path_intermediate_texture
            .as_ref()
            .context("missing path intermediate texture")?;

        let mut vertices = Vec::new();
        for path in paths {
            vertices.extend(path.vertices.iter().map(|v| PathRasterizationVertex {
                xy_position: v.xy_position,
                st_position: v.st_position,
                color: path.color,
                bounds: path.bounds.intersect(&path.content_mask.bounds),
            }));
        }
        let vertex_instance_bindings = writer.write(&vertices)?;

        let render_pass_descriptor = metal::RenderPassDescriptor::new();
        let color_attachment = render_pass_descriptor
            .color_attachments()
            .object_at(0)
            .unwrap();
        color_attachment.set_load_action(metal::MTLLoadAction::Clear);
        color_attachment.set_clear_color(metal::MTLClearColor::new(0., 0., 0., 0.));

        if let Some(msaa_texture) = &self.path_intermediate_msaa_texture {
            color_attachment.set_texture(Some(msaa_texture));
            color_attachment.set_resolve_texture(Some(intermediate_texture));
            color_attachment.set_store_action(metal::MTLStoreAction::MultisampleResolve);
        } else {
            color_attachment.set_texture(Some(intermediate_texture));
            color_attachment.set_store_action(metal::MTLStoreAction::Store);
        }

        let command_encoder = command_buffer.new_render_command_encoder(render_pass_descriptor);
        command_encoder.set_render_pipeline_state(&self.paths_rasterization_pipeline_state);
        command_encoder.set_vertex_buffer(
            PathRasterizationInputIndex::Vertices as u64,
            Some(&vertex_instance_bindings.buffer),
            vertex_instance_bindings.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            PathRasterizationInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_buffer(
            PathRasterizationInputIndex::Vertices as u64,
            Some(&vertex_instance_bindings.buffer),
            vertex_instance_bindings.offset as u64,
        );
        // KOAN: colour slots (#1859).
        command_encoder.set_fragment_buffer(
            COLOR_SLOTS_INDEX,
            Some(&color_slots.buffer),
            color_slots.offset as u64,
        );
        command_encoder.draw_primitives(
            metal::MTLPrimitiveType::Triangle,
            0,
            vertices.len() as u64,
        );

        command_encoder.end_encoding();
        Ok(true)
    }

    fn draw_shadows(
        &self,
        shadows: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if shadows.is_empty() {
            return;
        }

        command_encoder.set_render_pipeline_state(&self.shadows_pipeline_state);
        // KOAN: colour slots (#1859).
        command_encoder.set_vertex_buffer(
            COLOR_SLOTS_INDEX,
            Some(&instance_bindings.color_slots.buffer),
            instance_bindings.color_slots.offset as u64,
        );
        command_encoder.set_vertex_buffer(
            ShadowInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            ShadowInputIndex::Shadows as u64,
            Some(&instance_bindings.shadows.buffer),
            instance_bindings.shadows.offset as u64,
        );
        command_encoder.set_fragment_buffer(
            ShadowInputIndex::Shadows as u64,
            Some(&instance_bindings.shadows.buffer),
            instance_bindings.shadows.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            ShadowInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            shadows.len() as u64,
            shadows.start as u64,
        );
    }

    // KOAN: shader quads (#1641). Composes the scene's layers and shaders
    // into `texture`, drawing the layers first if this scene's are not drawn
    // yet, with the shaders' uniforms as they are now.
    fn compose_frame(
        &mut self,
        scene: &Scene,
        texture: &metal::TextureRef,
        viewport_size: Size<DevicePixels>,
    ) -> Result<metal::CommandBuffer> {
        let plan = scene.shader_layers.as_deref().context("no shader layers")?;
        let width = i32::from(viewport_size.width) as u64;
        let height = i32::from(viewport_size.height) as u64;
        if self.blank_texture.is_none() {
            let descriptor = metal::TextureDescriptor::new();
            descriptor.set_width(1);
            descriptor.set_height(1);
            descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            descriptor.set_storage_mode(metal::MTLStorageMode::Private);
            self.blank_texture = Some(self.device.new_texture(&descriptor));
        }
        if self.compose_pipelines.is_none() {
            self.compose_pipelines = Some(ComposePipelines::new(&self.device)?);
        }
        // KOAN: (#1859) a layer drawing with a colour slot is drawn again
        // while one of its slots moves, when the table was written since it
        // was drawn, and once more after its slots stop, so the last value
        // lands; the other layers are kept.
        let now = gpui::present_now();
        let slot_generation = gpui::color_slots_generation();
        let held = self.shader_layers.holds(scene.generation, width, height)
            && self.shader_layers.layers.len() == plan.layers.len();
        if !held {
            let mut pool = self.shader_layers.take(width, height);
            let mut drawn = Vec::with_capacity(plan.layers.len());
            for (k, layer) in plan.layers.iter().enumerate() {
                if layer.is_blank() {
                    drawn.push(None);
                    continue;
                }
                let target = pool.pop().unwrap_or_else(|| {
                    let descriptor = metal::TextureDescriptor::new();
                    descriptor.set_width(width);
                    descriptor.set_height(height);
                    descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                    descriptor.set_usage(
                        metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead,
                    );
                    descriptor.set_storage_mode(metal::MTLStorageMode::Private);
                    self.device.new_texture(&descriptor)
                });
                // The same queue runs these before the compose that reads them.
                self.render_frame(layer, &target, viewport_size, k > 0)?
                    .commit();
                drawn.push(Some(target));
            }
            self.shader_layers = ShaderLayerTextures {
                generation: scene.generation,
                slot_generation,
                width,
                height,
                layers: drawn,
                moving: plan
                    .layers
                    .iter()
                    .map(|l| l.color_slots_moving(now))
                    .collect(),
            };
        } else {
            let written = self.shader_layers.slot_generation != slot_generation;
            for (k, layer) in plan.layers.iter().enumerate() {
                if layer.color_slots.is_empty() {
                    continue;
                }
                let moving = layer.color_slots_moving(now);
                let was_moving = std::mem::replace(&mut self.shader_layers.moving[k], moving);
                if !(written || moving || was_moving) {
                    continue;
                }
                if let Some(target) = self.shader_layers.layers[k].clone() {
                    // After the last compose that read it: the same queue.
                    self.render_frame(layer, &target, viewport_size, k > 0)?
                        .commit();
                }
            }
            self.shader_layers.slot_generation = slot_generation;
        }
        let pipelines = self
            .compose_pipelines
            .clone()
            .context("missing compose pipelines")?;

        let mut writer = InstanceBufferWriter::new(
            &self.device,
            &self.instance_buffer_pool,
            self.is_unified_memory,
        );
        // KOAN: the present clock (#1859).
        let now = gpui::present_now();
        let shaders = writer.write_iter(
            scene
                .shaders
                .iter()
                .map(|shader| gpui::ShaderInstance::of(shader, now)),
        )?;
        let command_buffer = self.command_queue.new_command_buffer().to_owned();
        let render_pass_descriptor = metal::RenderPassDescriptor::new();
        let color_attachment = render_pass_descriptor
            .color_attachments()
            .object_at(0)
            .unwrap();
        color_attachment.set_texture(Some(texture));
        color_attachment.set_store_action(metal::MTLStoreAction::Store);
        let first = self.shader_layers.layers.first().cloned().flatten();
        if first.is_some() {
            color_attachment.set_load_action(metal::MTLLoadAction::DontCare);
        } else {
            let alpha = if self.opaque { 1. } else { 0. };
            color_attachment.set_load_action(metal::MTLLoadAction::Clear);
            color_attachment.set_clear_color(metal::MTLClearColor::new(0., 0., 0., alpha));
        }
        let encoder = command_buffer.new_render_command_encoder(render_pass_descriptor);
        encoder.set_viewport(metal::MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: width as f64,
            height: height as f64,
            znear: 0.0,
            zfar: 1.0,
        });
        let layer = |encoder: &metal::RenderCommandEncoderRef,
                     pipeline: &metal::RenderPipelineState,
                     texture: &metal::Texture| {
            encoder.set_render_pipeline_state(pipeline);
            encoder.set_fragment_texture(0, Some(texture));
            encoder.draw_primitives(metal::MTLPrimitiveType::Triangle, 0, 3);
        };
        if let Some(first) = &first {
            layer(encoder, &pipelines.copy, first);
        }
        let bindings = ShaderBindings(shaders);
        for (k, group) in plan.groups.iter().enumerate() {
            let mut start = group.start;
            while start < group.end {
                let head = &scene.shaders[start];
                let key = |s: &gpui::PaintShader| {
                    (
                        s.program.name,
                        s.image.as_ref().map(|t| t.texture_id),
                        s.group,
                    )
                };
                let end = scene.shaders[start..group.end]
                    .iter()
                    .position(|s| key(s) != key(head) || s.group.is_some())
                    .map_or(group.end, |n| (start + n).max(start + 1));
                self.draw_shaders(
                    head.program,
                    head.image.as_ref().map(|t| t.texture_id),
                    head.group,
                    start..end,
                    &bindings,
                    viewport_size,
                    encoder,
                );
                start = end;
            }
            if let Some(Some(over)) = self.shader_layers.layers.get(k + 1) {
                layer(encoder, &pipelines.over, over);
            }
        }
        encoder.end_encoding();

        let instance_buffer_pool = self.instance_buffer_pool.clone();
        let instance_buffer = Cell::new(Some(writer.finish()));
        let block = RcBlock::new(move |buffer: ptr::NonNull<AnyObject>| {
            note_gpu_time(buffer); // KOAN: #2034.
            if let Some(instance_buffer) = instance_buffer.take() {
                instance_buffer_pool.lock().release(instance_buffer);
            }
        });
        // SAFETY: as in `render_frame`.
        unsafe {
            command_buffer.add_completed_handler(&*RcBlock::as_ptr(&block).cast());
        }
        Ok(command_buffer)
    }

    // KOAN: (#1859, #1878) each layer group's scene drawn into a texture of
    // its own: the visible part of the shader composing it, its content
    // moved to the texture's origin, or the window's size for one holding
    // paths or too large. A group is drawn again when the frame is new or
    // when `Scene::layer_group_stale` says so. Groups nested in others are
    // drawn first, as their composing shader is in the enclosing group's
    // scene.
    fn draw_layer_groups(
        &mut self,
        scene: &Scene,
        viewport_size: Size<DevicePixels>,
    ) -> Result<()> {
        if scene.layer_groups.is_empty() {
            self.group_textures.clear();
            self.group_moving.clear();
            return Ok(());
        }
        let now = gpui::present_now();
        let new_frame = self.group_generation != scene.generation;
        let slot_generation = gpui::color_slots_generation();
        let slots_written = self.group_slot_generation != slot_generation;
        let count = scene.layer_groups.len();
        if new_frame {
            self.group_moving.clear();
        }
        self.group_moving.resize(count, false);
        let mut redrawn = vec![false; count];
        let sizes: Vec<Size<DevicePixels>> = scene
            .layer_group_frames
            .iter()
            .map(|frame| match frame {
                Some(frame) => size(
                    DevicePixels((frame.size.width.0.ceil() as i32).max(1)),
                    DevicePixels((frame.size.height.0.ceil() as i32).max(1)),
                ),
                None => viewport_size,
            })
            .collect();
        let mut held = std::mem::take(&mut self.group_textures);
        held.resize(scene.layer_groups.len(), None);
        for index in (0..scene.layer_groups.len()).rev() {
            let group = &scene.layer_groups[index];
            if group.is_blank() {
                held[index] = None;
                continue;
            }
            let extent = sizes[index];
            let width = i32::from(extent.width) as u64;
            let height = i32::from(extent.height) as u64;
            let fits = held[index]
                .as_ref()
                .is_some_and(|t| t.width() == width && t.height() == height);
            let stale = scene.layer_group_stale(
                index,
                now,
                slots_written,
                &mut self.group_moving[index],
                &redrawn,
            );
            if fits && !new_frame && !stale {
                continue;
            }
            redrawn[index] = true;
            let target = match held[index].take().filter(|_| fits) {
                Some(target) => target,
                None => {
                    let descriptor = metal::TextureDescriptor::new();
                    descriptor.set_width(width);
                    descriptor.set_height(height);
                    descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                    descriptor.set_usage(
                        metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead,
                    );
                    descriptor.set_storage_mode(metal::MTLStorageMode::Private);
                    self.device.new_texture(&descriptor)
                }
            };
            // Drawn with the groups so far in place, which a group nested in
            // this one is (higher indices first). The same queue runs these
            // before whatever samples them.
            self.group_textures = held.clone();
            self.render_frame(group, &target, extent, true)?.commit();
            held[index] = Some(target);
        }
        self.group_textures = held;
        self.group_generation = scene.generation;
        self.group_slot_generation = slot_generation;
        Ok(())
    }

    // KOAN: shader quads (#1641). A program's pipeline is compiled from its
    // source the first time it is drawn: the app's `shade` between a prelude
    // declaring the instance and a vertex and fragment function calling it.
    fn shader_pipeline(
        &self,
        program: &'static gpui::ShaderProgram,
    ) -> Option<metal::RenderPipelineState> {
        let mut pipelines = self.shader_pipelines.borrow_mut();
        pipelines
            .entry(program.name)
            .or_insert_with(|| {
                let source = format!("{SHADER_PRELUDE}\n{}\n{SHADER_MAIN}", program.msl);
                match self
                    .device
                    .new_library_with_source(&source, &metal::CompileOptions::new())
                {
                    Ok(library) => Some(build_pipeline_state(
                        &self.device,
                        &library,
                        program.name,
                        "koan_shader_vertex",
                        "koan_shader_fragment",
                        MTLPixelFormat::BGRA8Unorm,
                    )),
                    Err(error) => {
                        log::error!("shader {} did not compile: {error}", program.name);
                        None
                    }
                }
            })
            .clone()
    }

    // KOAN: shader quads (#1641).
    fn draw_shaders(
        &self,
        program: &'static gpui::ShaderProgram,
        texture: Option<AtlasTextureId>,
        group: Option<usize>,
        shaders: Range<usize>,
        instance_bindings: &impl HasShaders,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        let binding = instance_bindings.shaders();
        if shaders.is_empty() {
            return;
        }
        let Some(pipeline) = self.shader_pipeline(program) else {
            return;
        };
        command_encoder.set_render_pipeline_state(&pipeline);
        command_encoder.set_vertex_buffer(0, Some(&self.unit_vertices), 0);
        command_encoder.set_vertex_buffer(1, Some(&binding.buffer), binding.offset as u64);
        command_encoder.set_fragment_buffer(1, Some(&binding.buffer), binding.offset as u64);
        // KOAN: a layer group's texture, drawn this frame (#1859).
        let image = group
            .and_then(|group| self.group_textures.get(group).cloned().flatten())
            .or_else(|| texture.and_then(|id| self.sprite_atlas.metal_texture(id)))
            .or_else(|| self.blank_texture.clone());
        command_encoder.set_fragment_texture(0, image.as_deref());
        let viewport = [
            i32::from(viewport_size.width) as f32,
            i32::from(viewport_size.height) as f32,
        ];
        command_encoder.set_vertex_bytes(
            2,
            mem::size_of_val(&viewport) as u64,
            viewport.as_ptr() as *const c_void,
        );
        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            shaders.len() as u64,
            shaders.start as u64,
        );
    }

    fn draw_quads(
        &self,
        quads: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if quads.is_empty() {
            return;
        }

        command_encoder.set_render_pipeline_state(&self.quads_pipeline_state);
        // KOAN: colour slots (#1859).
        command_encoder.set_vertex_buffer(
            COLOR_SLOTS_INDEX,
            Some(&instance_bindings.color_slots.buffer),
            instance_bindings.color_slots.offset as u64,
        );
        command_encoder.set_vertex_buffer(
            QuadInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            QuadInputIndex::Quads as u64,
            Some(&instance_bindings.quads.buffer),
            instance_bindings.quads.offset as u64,
        );
        command_encoder.set_fragment_buffer(
            QuadInputIndex::Quads as u64,
            Some(&instance_bindings.quads.buffer),
            instance_bindings.quads.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            QuadInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            quads.len() as u64,
            quads.start as u64,
        );
    }

    fn draw_paths_from_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        writer: &mut InstanceBufferWriter,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) -> Result<()> {
        let Some(first_path) = paths.first() else {
            return Ok(());
        };
        let intermediate_texture = self
            .path_intermediate_texture
            .as_ref()
            .context("missing path intermediate texture")?;

        command_encoder.set_render_pipeline_state(&self.path_sprites_pipeline_state);
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        command_encoder.set_fragment_texture(
            SpriteInputIndex::AtlasTexture as u64,
            Some(intermediate_texture),
        );

        // When copying paths from the intermediate texture to the drawable,
        // each pixel must only be copied once, in case of transparent paths.
        //
        // If all paths have the same draw order, then their bounds are all
        // disjoint, so we can copy each path's bounds individually. If this
        // batch combines different draw orders, we perform a single copy
        // for a minimal spanning rect.
        let sprites;
        if paths.last().unwrap().order == first_path.order {
            sprites = paths
                .iter()
                .map(|path| PathSprite {
                    bounds: path.clipped_bounds(),
                })
                .collect();
        } else {
            let mut bounds = first_path.clipped_bounds();
            for path in paths.iter().skip(1) {
                bounds = bounds.union(&path.clipped_bounds());
            }
            sprites = vec![PathSprite { bounds }];
        }

        let sprite_instance_bindings = writer.write(&sprites)?;
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&sprite_instance_bindings.buffer),
            sprite_instance_bindings.offset as u64,
        );

        command_encoder.draw_primitives_instanced(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            sprites.len() as u64,
        );
        Ok(())
    }

    fn draw_underlines(
        &self,
        underlines: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if underlines.is_empty() {
            return;
        }

        command_encoder.set_render_pipeline_state(&self.underlines_pipeline_state);
        // KOAN: colour slots (#1859).
        command_encoder.set_vertex_buffer(
            COLOR_SLOTS_INDEX,
            Some(&instance_bindings.color_slots.buffer),
            instance_bindings.color_slots.offset as u64,
        );
        command_encoder.set_vertex_buffer(
            UnderlineInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            UnderlineInputIndex::Underlines as u64,
            Some(&instance_bindings.underlines.buffer),
            instance_bindings.underlines.offset as u64,
        );
        command_encoder.set_fragment_buffer(
            UnderlineInputIndex::Underlines as u64,
            Some(&instance_bindings.underlines.buffer),
            instance_bindings.underlines.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            UnderlineInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            underlines.len() as u64,
            underlines.start as u64,
        );
    }

    fn draw_monochrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if sprites.is_empty() {
            return;
        }

        let Some(texture) = self.sprite_atlas.metal_texture(texture_id) else {
            return;
        };
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.set_render_pipeline_state(&self.monochrome_sprites_pipeline_state);
        // KOAN: colour slots (#1859).
        command_encoder.set_vertex_buffer(
            COLOR_SLOTS_INDEX,
            Some(&instance_bindings.color_slots.buffer),
            instance_bindings.color_slots.offset as u64,
        );
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_bindings.monochrome_sprites.buffer),
            instance_bindings.monochrome_sprites.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::AtlasTextureSize as u64,
            mem::size_of_val(&texture_size) as u64,
            &texture_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_bindings.monochrome_sprites.buffer),
            instance_bindings.monochrome_sprites.offset as u64,
        );
        command_encoder.set_fragment_texture(SpriteInputIndex::AtlasTexture as u64, Some(&texture));

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            sprites.len() as u64,
            sprites.start as u64,
        );
    }

    fn draw_polychrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if sprites.is_empty() {
            return;
        }

        let Some(texture) = self.sprite_atlas.metal_texture(texture_id) else {
            return;
        };
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.set_render_pipeline_state(&self.polychrome_sprites_pipeline_state);
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_bindings.polychrome_sprites.buffer),
            instance_bindings.polychrome_sprites.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_vertex_bytes(
            SpriteInputIndex::AtlasTextureSize as u64,
            mem::size_of_val(&texture_size) as u64,
            &texture_size as *const Size<DevicePixels> as *const _,
        );
        command_encoder.set_fragment_buffer(
            SpriteInputIndex::Sprites as u64,
            Some(&instance_bindings.polychrome_sprites.buffer),
            instance_bindings.polychrome_sprites.offset as u64,
        );
        command_encoder.set_fragment_texture(SpriteInputIndex::AtlasTexture as u64, Some(&texture));

        command_encoder.draw_primitives_instanced_base_instance(
            metal::MTLPrimitiveType::Triangle,
            0,
            6,
            sprites.len() as u64,
            sprites.start as u64,
        );
    }

    fn draw_surfaces(
        &mut self,
        surfaces: &[PaintSurface],
        first_surface: usize,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &metal::RenderCommandEncoderRef,
    ) {
        if surfaces.is_empty() {
            return;
        }

        command_encoder.set_render_pipeline_state(&self.surfaces_pipeline_state);
        command_encoder.set_vertex_buffer(
            SurfaceInputIndex::Vertices as u64,
            Some(&self.unit_vertices),
            0,
        );
        command_encoder.set_vertex_buffer(
            SurfaceInputIndex::Surfaces as u64,
            Some(&instance_bindings.surfaces.buffer),
            instance_bindings.surfaces.offset as u64,
        );
        command_encoder.set_vertex_bytes(
            SurfaceInputIndex::ViewportSize as u64,
            mem::size_of_val(&viewport_size) as u64,
            &viewport_size as *const Size<DevicePixels> as *const _,
        );

        for (index, surface) in surfaces.iter().enumerate() {
            let image_buffer = &surface.image_buffer;
            let texture_size = size(
                DevicePixels::from(CVPixelBufferGetWidth(image_buffer) as i32),
                DevicePixels::from(CVPixelBufferGetHeight(image_buffer) as i32),
            );

            assert_eq!(
                CVPixelBufferGetPixelFormatType(image_buffer),
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
            );

            let plane_texture = |plane, format| unsafe {
                let mut texture = ptr::null_mut();
                let status = CVMetalTextureCache::create_texture_from_image(
                    None,
                    &self.core_video_texture_cache,
                    image_buffer,
                    None,
                    format,
                    CVPixelBufferGetWidthOfPlane(image_buffer, plane),
                    CVPixelBufferGetHeightOfPlane(image_buffer, plane),
                    plane,
                    ptr::NonNull::from(&mut texture),
                );
                assert_eq!(
                    status, kCVReturnSuccess,
                    "failed to create CoreVideo Metal texture"
                );
                CFRetained::<CVMetalTexture>::from_raw(
                    ptr::NonNull::new(texture).expect("CoreVideo returned a null texture"),
                )
            };
            let y_texture = plane_texture(0, objc2_metal::MTLPixelFormat::R8Unorm);
            let cb_cr_texture = plane_texture(1, objc2_metal::MTLPixelFormat::RG8Unorm);

            command_encoder.set_vertex_bytes(
                SurfaceInputIndex::TextureSize as u64,
                mem::size_of_val(&texture_size) as u64,
                &texture_size as *const Size<DevicePixels> as *const _,
            );
            command_encoder.set_fragment_texture(SurfaceInputIndex::YTexture as u64, unsafe {
                let texture = CVMetalTextureGetTexture(&y_texture)
                    .expect("CoreVideo returned no Metal texture");
                Some(metal::TextureRef::from_ptr(
                    objc2::rc::Retained::as_ptr(&texture) as *mut _,
                ))
            });
            command_encoder.set_fragment_texture(SurfaceInputIndex::CbCrTexture as u64, unsafe {
                let texture = CVMetalTextureGetTexture(&cb_cr_texture)
                    .expect("CoreVideo returned no Metal texture");
                Some(metal::TextureRef::from_ptr(
                    objc2::rc::Retained::as_ptr(&texture) as *mut _,
                ))
            });

            command_encoder.draw_primitives_instanced_base_instance(
                metal::MTLPrimitiveType::Triangle,
                0,
                6,
                1,
                (first_surface + index) as u64,
            );
        }
    }
}

fn new_command_encoder_for_texture<'a>(
    command_buffer: &'a metal::CommandBufferRef,
    texture: &'a metal::TextureRef,
    viewport_size: Size<DevicePixels>,
    clear_color: Option<metal::MTLClearColor>,
) -> &'a metal::RenderCommandEncoderRef {
    let render_pass_descriptor = metal::RenderPassDescriptor::new();
    let color_attachment = render_pass_descriptor
        .color_attachments()
        .object_at(0)
        .unwrap();
    color_attachment.set_texture(Some(texture));
    color_attachment.set_store_action(metal::MTLStoreAction::Store);
    if let Some(clear_color) = clear_color {
        color_attachment.set_load_action(metal::MTLLoadAction::Clear);
        color_attachment.set_clear_color(clear_color);
    } else {
        color_attachment.set_load_action(metal::MTLLoadAction::Load);
    }

    let command_encoder = command_buffer.new_render_command_encoder(render_pass_descriptor);
    command_encoder.set_viewport(metal::MTLViewport {
        originX: 0.0,
        originY: 0.0,
        width: i32::from(viewport_size.width) as f64,
        height: i32::from(viewport_size.height) as f64,
        znear: 0.0,
        zfar: 1.0,
    });
    command_encoder
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
fn read_texture_to_image(texture: &metal::TextureRef) -> Result<RgbaImage> {
    let width = texture.width() as u32;
    let height = texture.height() as u32;
    let bytes_per_row = width as usize * 4;
    let mut pixels = vec![0u8; height as usize * bytes_per_row];

    let region = metal::MTLRegion {
        origin: metal::MTLOrigin { x: 0, y: 0, z: 0 },
        size: metal::MTLSize {
            width: width as u64,
            height: height as u64,
            depth: 1,
        },
    };
    texture.get_bytes(
        pixels.as_mut_ptr() as *mut std::ffi::c_void,
        bytes_per_row as u64,
        region,
        0,
    );

    // Convert BGRA to RGBA (swap B and R channels)
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }

    RgbaImage::from_raw(width, height, pixels).context("failed to create RgbaImage from pixel data")
}

fn build_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
) -> metal::RenderPipelineState {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)
        .expect("error locating vertex function");
    let fragment_fn = library
        .get_function(fragment_fn_name, None)
        .expect("error locating fragment function");

    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(true);
    color_attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::SourceAlpha);
    color_attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    device
        .new_render_pipeline_state(&descriptor)
        .expect("could not create render pipeline state")
}

fn build_path_sprite_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
) -> metal::RenderPipelineState {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)
        .expect("error locating vertex function");
    let fragment_fn = library
        .get_function(fragment_fn_name, None)
        .expect("error locating fragment function");

    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(true);
    color_attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    device
        .new_render_pipeline_state(&descriptor)
        .expect("could not create render pipeline state")
}

fn build_path_rasterization_pipeline_state(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: metal::MTLPixelFormat,
    path_sample_count: u32,
) -> metal::RenderPipelineState {
    let vertex_fn = library
        .get_function(vertex_fn_name, None)
        .expect("error locating vertex function");
    let fragment_fn = library
        .get_function(fragment_fn_name, None)
        .expect("error locating fragment function");

    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label(label);
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    if path_sample_count > 1 {
        descriptor.set_raster_sample_count(path_sample_count as _);
        descriptor.set_alpha_to_coverage_enabled(false);
    }
    let color_attachment = descriptor.color_attachments().object_at(0).unwrap();
    color_attachment.set_pixel_format(pixel_format);
    color_attachment.set_blending_enabled(true);
    color_attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
    color_attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color_attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    device
        .new_render_pipeline_state(&descriptor)
        .expect("could not create render pipeline state")
}

#[derive(Clone)]
struct InstanceBinding {
    buffer: metal::Buffer,
    offset: usize,
}

struct InstanceBindings {
    quads: InstanceBinding,
    shadows: InstanceBinding,
    underlines: InstanceBinding,
    monochrome_sprites: InstanceBinding,
    polychrome_sprites: InstanceBinding,
    surfaces: InstanceBinding,
    shaders: InstanceBinding,     // KOAN: #1641.
    color_slots: InstanceBinding, // KOAN: #1859.
}

// KOAN: the vertex buffer the stock shaders read the colour slots from
// (#1859).
const COLOR_SLOTS_INDEX: u64 = 20;

// KOAN: (#1641) the layers of the last frame drawn with shaders, `None`
// for a blank one, kept while presents compose them.
#[derive(Default)]
struct ShaderLayerTextures {
    generation: u64,
    slot_generation: u64, // KOAN: #1859.
    width: u64,
    height: u64,
    layers: Vec<Option<metal::Texture>>,
    /// KOAN: whether each layer's colour slots were moving when it was
    /// last drawn (#1859).
    moving: Vec<bool>,
}

impl ShaderLayerTextures {
    fn holds(&self, generation: u64, width: u64, height: u64) -> bool {
        self.generation == generation
            && self.width == width
            && self.height == height
            && !self.layers.is_empty()
    }

    /// The textures, to be drawn again, where they are the size wanted.
    fn take(&mut self, width: u64, height: u64) -> Vec<metal::Texture> {
        let same = self.width == width && self.height == height;
        let layers = std::mem::take(&mut self.layers);
        if same {
            layers.into_iter().flatten().collect()
        } else {
            Vec::new()
        }
    }

    fn release(&mut self) {
        *self = Self::default();
    }
}

// KOAN: where a frame's shader instances are, whether written with the whole
// scene or alone for a compose (#1641).
trait HasShaders {
    fn shaders(&self) -> &InstanceBinding;
}

impl HasShaders for InstanceBindings {
    fn shaders(&self) -> &InstanceBinding {
        &self.shaders
    }
}

struct ShaderBindings(InstanceBinding);

impl HasShaders for ShaderBindings {
    fn shaders(&self) -> &InstanceBinding {
        &self.0
    }
}

// KOAN: (#1641) draws a layer into the drawable as one triangle covering
// it, read texel for texel: unblended for the first, and over what is there
// for the rest, whose colours are premultiplied, as drawing over a clear
// layer leaves them.
const COMPOSE_SOURCE: &str = r#"
#include <metal_stdlib>
using namespace metal;

vertex float4 koan_compose_vertex(uint id [[vertex_id]]) {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * 2.0 - 1.0, 0.0, 1.0);
}

fragment float4 koan_compose_fragment(
    float4 position [[position]],
    texture2d<float, access::read> layer [[texture(0)]]
) {
    return layer.read(uint2(position.xy));
}
"#;

#[derive(Clone)]
struct ComposePipelines {
    copy: metal::RenderPipelineState,
    over: metal::RenderPipelineState,
}

impl ComposePipelines {
    fn new(device: &metal::Device) -> Result<Self> {
        let library = device
            .new_library_with_source(COMPOSE_SOURCE, &metal::CompileOptions::new())
            .map_err(|error| anyhow::anyhow!("compose shader: {error}"))?;
        let vertex = library
            .get_function("koan_compose_vertex", None)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let fragment = library
            .get_function("koan_compose_fragment", None)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let pipeline = |over: bool| {
            let descriptor = metal::RenderPipelineDescriptor::new();
            descriptor.set_label(if over {
                "koan_compose_over"
            } else {
                "koan_compose_copy"
            });
            descriptor.set_vertex_function(Some(&vertex));
            descriptor.set_fragment_function(Some(&fragment));
            let color = descriptor.color_attachments().object_at(0).unwrap();
            color.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            color.set_blending_enabled(over);
            if over {
                color.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
                color.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
                color.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
                color.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
                color.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
                color
                    .set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
            }
            device
                .new_render_pipeline_state(&descriptor)
                .map_err(|error| anyhow::anyhow!("compose pipeline: {error}"))
        };
        Ok(Self {
            copy: pipeline(false)?,
            over: pipeline(true)?,
        })
    }
}

// KOAN: shader quads (#1641): what a program's `shade` is compiled between.
const SHADER_PRELUDE: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct KoanShaderInstance {
    float4 bounds;
    float4 clip;
    float4 uniforms[20];
    float4 time;
    float4 tile;
};

// KOAN: tracks (#1859), evaluated as `Track::at` does in gpui's shader.rs.
float koan_bezier_curve(float a, float b, float s) {
    float u = 1.0 - s;
    return 3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s;
}

float koan_bezier(float x1, float y1, float x2, float y2, float t) {
    if (t <= 0.0 || t >= 1.0) {
        return clamp(t, 0.0, 1.0);
    }
    float s = t;
    for (int i = 0; i < 6; i++) {
        float u = 1.0 - s;
        float d = 3.0 * u * u * x1 + 6.0 * u * s * (x2 - x1) + 3.0 * s * s * (1.0 - x2);
        if (abs(d) < 1e-5) {
            break;
        }
        s = clamp(s - (koan_bezier_curve(x1, x2, s) - t) / d, 0.0, 1.0);
    }
    return koan_bezier_curve(y1, y2, s);
}

float koan_spring(float x0, float v0, float k, float c, float t) {
    float w = sqrt(max(k, 1e-3));
    float z = c / (2.0 * w);
    if (z < 1.0) {
        float wd = w * sqrt(1.0 - z * z);
        return exp(-z * w * t) * (x0 * cos(wd * t) + (v0 + z * w * x0) / wd * sin(wd * t));
    }
    if (z > 1.0001) {
        float root = sqrt(z * z - 1.0);
        float r1 = -w * (z - root);
        float r2 = -w * (z + root);
        float c1 = (v0 - r2 * x0) / (r1 - r2);
        return c1 * exp(r1 * t) + (x0 - c1) * exp(r2 * t);
    }
    return (x0 + (v0 + w * x0) * t) * exp(-w * t);
}

// Track `k` of the uniforms at `time`.
float koan_track(constant float *u, uint k, float time) {
    constant float *t = u + 16 + k * 8;
    float from = t[0];
    float to = t[1];
    float length = t[3];
    if (length <= 0.0) {
        return to;
    }
    float e = max(time - t[2], 0.0);
    uint code = uint(t[4]);
    uint kind = code % 4;
    uint repeat = code / 4;
    if (kind == 2) {
        return e >= length ? to : to + koan_spring(from - to, t[7], t[5], t[6], e);
    }
    float p = e / length;
    if (repeat == 0) {
        p = min(p, 1.0);
    } else if (repeat == 1) {
        p = fract(p);
    } else {
        float q = fmod(p, 2.0);
        p = q > 1.0 ? 2.0 - q : q;
    }
    if (kind == 1) {
        half2 a = as_type<half2>(t[5]);
        half2 b = as_type<half2>(t[6]);
        p = koan_bezier(a.x, a.y, b.x, b.y, p);
    }
    return from + (to - from) * p;
}

// The shader's image at `uv`, 0 to 1 across it, filtered and kept off the
// texels around its tile in the atlas. Past the texture's edge it is clear,
// as a layer group's texture holds only its visible part (#1878).
float4 koan_sample(texture2d<float> image, float4 tile, float2 uv) {
    constexpr sampler linear(filter::linear, address::clamp_to_edge);
    float2 extent = float2(image.get_width(), image.get_height());
    float2 raw = tile.xy + uv * tile.zw;
    if (any(raw < 0.0) || any(raw > extent)) {
        return float4(0.0);
    }
    float2 at = clamp(
        raw,
        tile.xy + 0.5,
        tile.xy + tile.zw - 0.5
    );
    return image.sample(linear, at / extent);
}

struct KoanShaderVarying {
    float4 position [[position]];
    float2 local;
    uint instance [[flat]];
};
"#;

const SHADER_MAIN: &str = r#"
vertex KoanShaderVarying koan_shader_vertex(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    constant float2 *unit [[buffer(0)]],
    constant KoanShaderInstance *instances [[buffer(1)]],
    constant float2 *viewport [[buffer(2)]]
) {
    KoanShaderInstance s = instances[instance_id];
    float2 u = unit[vertex_id];
    float2 p = s.bounds.xy + u * s.bounds.zw;
    float2 ndc = p / *viewport * float2(2.0, -2.0) + float2(-1.0, 1.0);
    KoanShaderVarying out;
    out.position = float4(ndc, 0.0, 1.0);
    out.local = u * s.bounds.zw;
    out.instance = instance_id;
    return out;
}

fragment float4 koan_shader_fragment(
    KoanShaderVarying in [[stage_in]],
    constant KoanShaderInstance *instances [[buffer(1)]],
    texture2d<float> image [[texture(0)]]
) {
    constant KoanShaderInstance &s = instances[in.instance];
    float2 p = in.position.xy;
    if (any(p < s.clip.xy) || any(p > s.clip.xy + s.clip.zw)) {
        discard_fragment();
    }
    return shade(in.local, s.bounds.zw, s.time.x, (constant float *)s.uniforms, image, s.tile);
}
"#;

fn write_instances(scene: &Scene, writer: &mut InstanceBufferWriter) -> Result<InstanceBindings> {
    Ok(InstanceBindings {
        quads: writer.write(&scene.quads)?,
        shadows: writer.write(&scene.shadows)?,
        underlines: writer.write(&scene.underlines)?,
        monochrome_sprites: writer.write(&scene.monochrome_sprites)?,
        polychrome_sprites: writer.write(&scene.polychrome_sprites)?,
        surfaces: writer.write_iter(scene.surfaces.iter().map(|surface| SurfaceBounds {
            bounds: surface.bounds,
            content_mask: surface.content_mask,
        }))?,
        // KOAN: the shaders' uniforms as they are now, when the frame is
        // presented (#1641).
        shaders: {
            // KOAN: the present clock (#1859).
            let now = gpui::present_now();
            writer.write_iter(
                scene
                    .shaders
                    .iter()
                    .map(|shader| gpui::ShaderInstance::of(shader, now)),
            )?
        },
        // KOAN: the colour slots as they stand when the frame is presented
        // (#1859).
        color_slots: {
            thread_local! {
                static TABLE: std::cell::RefCell<Vec<f32>> = const { std::cell::RefCell::new(Vec::new()) };
            }
            TABLE.with(|table| {
                let mut table = table.borrow_mut();
                gpui::color_slots_frame(gpui::present_now(), &mut table);
                writer.write(&table)
            })?
        },
    })
}

struct InstanceBufferWriter {
    device: metal::Device,
    pool: Arc<Mutex<InstanceBufferPool>>,
    unified_memory: bool,
    filled: Vec<(InstanceBuffer, usize)>,
    current: InstanceBuffer,
    offset: usize,
}

impl InstanceBufferWriter {
    fn new(
        device: &metal::Device,
        pool: &Arc<Mutex<InstanceBufferPool>>,
        unified_memory: bool,
    ) -> Self {
        let current = pool.lock().acquire(device, unified_memory);
        Self {
            device: device.clone(),
            pool: pool.clone(),
            unified_memory,
            filled: Vec::new(),
            current,
            offset: 0,
        }
    }

    fn allocate<T>(&mut self, count: usize) -> Result<(InstanceBinding, &mut [MaybeUninit<T>])> {
        let size = mem::size_of::<T>() * count;
        let mut offset = self.offset.next_multiple_of(INSTANCE_BUFFER_ALIGNMENT);
        if offset + size > self.current.size {
            self.grow(size)?;
            offset = 0;
        }
        self.offset = offset + size;

        let binding = InstanceBinding {
            buffer: self.current.metal_buffer.clone(),
            offset,
        };
        // Safety: the reservation lies within a buffer this frame owns
        // exclusively, and never overlaps one handed out earlier.
        let values = unsafe {
            let start = (self.current.metal_buffer.contents() as *mut u8).add(offset);
            slice::from_raw_parts_mut(start.cast::<MaybeUninit<T>>(), count)
        };
        Ok((binding, values))
    }

    fn write<T>(&mut self, values: &[T]) -> Result<InstanceBinding> {
        let (binding, destination) = self.allocate::<T>(values.len())?;
        unsafe {
            ptr::copy_nonoverlapping(
                values.as_ptr(),
                destination.as_mut_ptr().cast::<T>(),
                values.len(),
            );
        }
        Ok(binding)
    }

    fn write_iter<T>(
        &mut self,
        values: impl ExactSizeIterator<Item = T>,
    ) -> Result<InstanceBinding> {
        let (binding, destination) = self.allocate::<T>(values.len())?;
        for (slot, value) in destination.iter_mut().zip(values) {
            slot.write(value);
        }
        Ok(binding)
    }

    fn grow(&mut self, required: usize) -> Result<()> {
        let mut pool = self.pool.lock();
        let buffer_size = (pool.buffer_size * 2)
            .max(required.next_power_of_two())
            .min(MAX_INSTANCE_BUFFER_SIZE);
        anyhow::ensure!(
            buffer_size >= required,
            "instance buffer needs {required} bytes, above the maximum of {MAX_INSTANCE_BUFFER_SIZE}"
        );
        anyhow::ensure!(
            buffer_size > self.current.size,
            "frame instance data exceeds the {MAX_INSTANCE_BUFFER_SIZE}-byte maximum"
        );
        if buffer_size != pool.buffer_size {
            log::info!("increased instance buffer size to {buffer_size}");
            pool.reset(buffer_size);
        }
        let buffer = pool.acquire(&self.device, self.unified_memory);
        drop(pool);

        let filled = mem::replace(&mut self.current, buffer);
        self.filled.push((filled, self.offset));
        self.offset = 0;
        Ok(())
    }

    fn finish(self) -> InstanceBuffer {
        let Self {
            unified_memory,
            filled,
            current,
            offset,
            ..
        } = self;

        if !unified_memory {
            for (buffer, written) in &filled {
                if *written == 0 {
                    continue;
                }
                buffer.metal_buffer.did_modify_range(NSRange {
                    location: 0,
                    length: *written as NSUInteger,
                });
            }
            if offset > 0 {
                current.metal_buffer.did_modify_range(NSRange {
                    location: 0,
                    length: offset as NSUInteger,
                });
            }
        }

        // Metal retains encoded resources until the command buffer completes.
        // Only the final, largest buffer is worth keeping in the pool.
        drop(filled);
        current
    }
}

#[repr(C)]
enum ShadowInputIndex {
    Vertices = 0,
    Shadows = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum QuadInputIndex {
    Vertices = 0,
    Quads = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum UnderlineInputIndex {
    Vertices = 0,
    Underlines = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum SpriteInputIndex {
    Vertices = 0,
    Sprites = 1,
    ViewportSize = 2,
    AtlasTextureSize = 3,
    AtlasTexture = 4,
}

#[repr(C)]
enum SurfaceInputIndex {
    Vertices = 0,
    Surfaces = 1,
    ViewportSize = 2,
    TextureSize = 3,
    YTexture = 4,
    CbCrTexture = 5,
}

#[repr(C)]
enum PathRasterizationInputIndex {
    Vertices = 0,
    ViewportSize = 1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PathSprite {
    pub bounds: Bounds<ScaledPixels>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct SurfaceBounds {
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
pub struct MetalHeadlessRenderer {
    renderer: MetalRenderer,
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
impl MetalHeadlessRenderer {
    pub fn new() -> Self {
        let instance_buffer_pool = Arc::new(Mutex::new(InstanceBufferPool::default()));
        let renderer = MetalRenderer::new_headless(instance_buffer_pool);
        Self { renderer }
    }
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
impl gpui::PlatformHeadlessRenderer for MetalHeadlessRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<image::RgbaImage> {
        self.renderer.render_scene_to_image(scene, size)
    }

    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> anyhow::Result<()> {
        self.renderer.render_scene(scene, size)
    }

    fn sprite_atlas(&self) -> Arc<dyn gpui::PlatformAtlas> {
        self.renderer.sprite_atlas().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::{CFDictionary, CFType};
    use objc2_core_video::{
        CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferIOSurfacePropertiesKey,
    };

    #[test]
    fn render_core_video_surface() {
        let attributes = CFDictionary::from_slices(
            &[unsafe { kCVPixelBufferIOSurfacePropertiesKey }],
            &[&*CFDictionary::<CFType, CFType>::empty()],
        );
        let mut image_buffer = ptr::null_mut();
        assert_eq!(
            unsafe {
                CVPixelBufferCreate(
                    None,
                    16,
                    16,
                    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                    Some(attributes.as_opaque()),
                    ptr::NonNull::from(&mut image_buffer),
                )
            },
            kCVReturnSuccess
        );
        let image_buffer =
            unsafe { CFRetained::from_raw(ptr::NonNull::new(image_buffer).unwrap()) };
        unsafe {
            assert_eq!(
                CVPixelBufferLockBaseAddress(&image_buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
            for plane in 0..2 {
                let bytes = CVPixelBufferGetBaseAddressOfPlane(&image_buffer, plane).cast::<u8>();
                let length = CVPixelBufferGetBytesPerRowOfPlane(&image_buffer, plane)
                    * CVPixelBufferGetHeightOfPlane(&image_buffer, plane);
                slice::from_raw_parts_mut(bytes, length).fill(128);
            }
            assert_eq!(
                CVPixelBufferUnlockBaseAddress(&image_buffer, CVPixelBufferLockFlags::empty()),
                kCVReturnSuccess
            );
        }

        let bounds = Bounds {
            origin: Point::default(),
            size: size(ScaledPixels(16.0), ScaledPixels(16.0)),
        };
        let mut scene = Scene::default();
        scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            image_buffer: image_buffer.clone(),
        });
        drop(image_buffer);
        scene.finish();

        let mut renderer = MetalRenderer::new_headless(Arc::new(Mutex::new(Default::default())));
        let image = renderer
            .render_scene_to_image(&scene, size(DevicePixels(16), DevicePixels(16)))
            .unwrap();
        for image::Rgba([red, green, blue, alpha]) in image.pixels() {
            assert!(red.abs_diff(128) <= 2);
            assert!(green.abs_diff(128) <= 2);
            assert!(blue.abs_diff(128) <= 2);
            assert_eq!(*alpha, 255);
        }
    }

    // KOAN: (#1859) the prelude's `koan_track` draws each curve where
    // `Track::at` says it stands, so a retarget on the CPU starts from what
    // is on screen.
    #[test]
    fn tracks_draw_where_the_cpu_says() {
        use gpui::{Track, TrackCurve, TrackRepeat};
        static PROGRAM: gpui::ShaderProgram = gpui::ShaderProgram {
            name: "koan-test-tracks",
            msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    uint k = uint(position.x / size.x * 5.0);
    return float4(koan_track(u, k, time), 0.0, 0.0, 1.0);
}
"#,
            wgsl: "",
            prepare: None,
        };
        let uniforms = Arc::new(gpui::ShaderUniforms::new());
        let now = uniforms.time_at(gpui::present_now());
        let track = |t: f32, length: f32, curve, repeat| Track {
            from: 0.1,
            to: 0.9,
            start: now - t,
            length,
            curve,
            repeat,
        };
        let tracks = [
            track(30.0, 100.0, TrackCurve::Linear, TrackRepeat::Once),
            track(
                20.0,
                100.0,
                TrackCurve::Bezier(0.2, 0.9, 0.3, 1.0),
                TrackRepeat::Once,
            ),
            track(
                0.3,
                5.0,
                TrackCurve::Spring {
                    stiffness: 30.0,
                    damping: 12.0,
                    velocity: 0.0,
                },
                TrackRepeat::Once,
            ),
            track(250.0, 100.0, TrackCurve::Linear, TrackRepeat::Loop),
            track(
                160.0,
                100.0,
                TrackCurve::Bezier(0.42, 0.0, 0.58, 1.0),
                TrackRepeat::Alternate,
            ),
        ];
        for (k, t) in tracks.iter().enumerate() {
            uniforms.set_track(k, *t);
        }
        let bounds = Bounds {
            origin: Point::default(),
            size: size(ScaledPixels(5.0), ScaledPixels(1.0)),
        };
        let mut scene = Scene::default();
        scene.insert_primitive(gpui::PaintShader {
            order: 0,
            bounds,
            content_mask: ContentMask { bounds },
            program: &PROGRAM,
            uniforms: uniforms.clone(),
            image: None,
            group: None,
            group_inset: Default::default(),
        });
        scene.finish();
        let mut renderer = MetalRenderer::new_headless(Arc::new(Mutex::new(Default::default())));
        // The render's moment falls between these two, the program compiled
        // in between: each track is drawn within what it passes through.
        let before = uniforms.time_at(gpui::present_now());
        let image = renderer
            .render_scene_to_image(&scene, size(DevicePixels(5), DevicePixels(1)))
            .unwrap();
        let after = uniforms.time_at(gpui::present_now());
        for (k, t) in tracks.iter().enumerate() {
            let steps = 64;
            let levels = (0..=steps).map(|i| {
                let at = before + (after - before) * i as f32 / steps as f32;
                (t.at(at) * 255.0).round() as i32
            });
            let (low, high) = levels.fold((i32::MAX, i32::MIN), |(l, h), v| (l.min(v), h.max(v)));
            let drawn = image.get_pixel(k as u32, 0).0[0] as i32;
            assert!(
                drawn >= low - 2 && drawn <= high + 2,
                "track {k}: drawn {drawn}, expected {low}..={high}"
            );
        }
    }

    // KOAN: (#1641) a scene with a shader nothing covers, presented as a
    // window presents it (the base kept, the shaders composed over it), is
    // the same picture as the whole scene drawn at once, and a re-present
    // reads the uniforms afresh.
    #[test]
    fn a_composed_present_matches_a_whole_render() {
        static PROGRAM: gpui::ShaderProgram = gpui::ShaderProgram {
            name: "koan-test-half",
            msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    return position.x < size.x * u[0] ? float4(1.0, 0.0, 0.0, 0.5) : float4(0.0);
}
"#,
            wgsl: "",
            prepare: None,
        };
        let rect = |x: f32, y: f32, w: f32, h: f32| Bounds {
            origin: Point::new(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(w), ScaledPixels(h)),
        };
        let uniforms = Arc::new(gpui::ShaderUniforms::new());
        uniforms.set(0, 0.5);
        let scene = |cover: bool| {
            let mut scene = Scene::default();
            let whole = rect(0.0, 0.0, 64.0, 32.0);
            scene.insert_primitive(gpui::Quad {
                bounds: whole,
                content_mask: ContentMask { bounds: whole },
                background: gpui::hsla(0.6, 0.5, 0.5, 1.0).into(),
                ..Default::default()
            });
            let shader = rect(8.0, 8.0, 32.0, 16.0);
            scene.insert_primitive(gpui::PaintShader {
                order: 0,
                bounds: shader,
                content_mask: ContentMask { bounds: shader },
                program: &PROGRAM,
                uniforms: uniforms.clone(),
                image: None,
                group: None,
                group_inset: Default::default(),
            });
            let over = if cover {
                rect(30.0, 10.0, 20.0, 4.0)
            } else {
                rect(50.0, 10.0, 10.0, 4.0)
            };
            scene.insert_primitive(gpui::Quad {
                bounds: over,
                content_mask: ContentMask { bounds: over },
                background: gpui::hsla(0.1, 0.8, 0.5, 1.0).into(),
                ..Default::default()
            });
            scene.finish();
            scene
        };
        let viewport = size(DevicePixels(64), DevicePixels(32));
        let mut renderer = MetalRenderer::new_headless(Arc::new(Mutex::new(Default::default())));
        // Layers over a shader are composed premultiplied in 8 bits, so a
        // translucent edge may round a level apart.
        let close = |a: &RgbaImage, b: &RgbaImage| {
            a.as_raw()
                .iter()
                .zip(b.as_raw())
                .all(|(x, y)| x.abs_diff(*y) <= 1)
        };
        for covered in [false, true] {
            let scene = scene(covered);
            let plan = scene.shader_layers.as_deref().expect("a plan");
            assert_eq!(plan.groups.len(), 1);
            assert_eq!(plan.layers[1].is_blank(), !covered);
            uniforms.set(0, 0.5);
            let whole = renderer.render_scene_to_image(&scene, viewport).unwrap();
            let composed = renderer
                .compose_scene_to_image(&scene, viewport, 2)
                .unwrap();
            assert!(close(&whole, &composed), "covered: {covered}");

            uniforms.set(0, 1.0);
            let moved = renderer
                .compose_scene_to_image(&scene, viewport, 1)
                .unwrap();
            assert_ne!(moved.as_raw(), composed.as_raw());
            let whole = renderer.render_scene_to_image(&scene, viewport).unwrap();
            assert!(close(&whole, &moved), "covered: {covered}, moved");
        }
    }

    // KOAN: (#1878) a layer group at the identity pose, partly outside the
    // window, composed through its texture gives the pixels its content does
    // drawn without a group: the offset, the clipped frame, the tile and the
    // texture's viewport agree.
    #[test]
    fn a_group_at_rest_matches_its_content() {
        static HALF: gpui::ShaderProgram = gpui::ShaderProgram {
            name: "koan-test-half",
            msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    return position.x < size.x * u[0] ? float4(1.0, 0.0, 0.0, 0.5) : float4(0.0);
}
"#,
            wgsl: "",
            prepare: None,
        };
        static IDENTITY: gpui::ShaderProgram = gpui::ShaderProgram {
            name: "koan-test-identity",
            msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float4 c = koan_sample(image, tile, position / size);
    float3 rgb = c.a > 0.0 ? c.rgb / c.a : float3(0.0);
    return float4(rgb, c.a);
}
"#,
            wgsl: "",
            prepare: None,
        };
        let rect = |x: f32, y: f32, w: f32, h: f32| Bounds {
            origin: Point::new(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(w), ScaledPixels(h)),
        };
        let quad = |at: Bounds<ScaledPixels>, hue: f32| gpui::Quad {
            bounds: at,
            content_mask: ContentMask { bounds: at },
            background: gpui::hsla(hue, 0.5, 0.5, 1.0).into(),
            ..Default::default()
        };
        let shader = |at: Bounds<ScaledPixels>, program| gpui::PaintShader {
            order: 0,
            bounds: at,
            content_mask: ContentMask { bounds: at },
            program,
            uniforms: Arc::new(gpui::ShaderUniforms::new()),
            image: None,
            group: None,
            group_inset: Default::default(),
        };
        let scene = |grouped: bool| {
            let mut scene = Scene::default();
            scene.insert_primitive(quad(rect(0.0, 0.0, 64.0, 32.0), 0.6));
            if grouped {
                scene.push_group();
            }
            scene.insert_primitive(quad(rect(10.0, 10.0, 20.0, 10.0), 0.3));
            let half = shader(rect(30.0, 8.0, 24.0, 16.0), &HALF);
            half.uniforms.set(0, 0.5);
            scene.insert_primitive(half);
            if grouped {
                let mut compose = shader(rect(4.0, 4.0, 80.0, 40.0), &IDENTITY);
                compose.content_mask = ContentMask {
                    bounds: rect(0.0, 0.0, 64.0, 32.0),
                };
                scene.pop_group(compose);
            }
            scene.finish();
            scene
        };
        let grouped = scene(true);
        assert_eq!(
            grouped.layer_group_frames[0],
            Some(rect(4.0, 4.0, 60.0, 28.0))
        );
        let viewport = size(DevicePixels(64), DevicePixels(32));
        let mut renderer = MetalRenderer::new_headless(Arc::new(Mutex::new(Default::default())));
        let plain = renderer
            .compose_scene_to_image(&scene(false), viewport, 1)
            .unwrap();
        let composed = renderer
            .compose_scene_to_image(&grouped, viewport, 1)
            .unwrap();
        let worst = plain
            .as_raw()
            .iter()
            .zip(composed.as_raw())
            .map(|(x, y)| x.abs_diff(*y))
            .max();
        assert!(worst <= Some(2), "worst channel difference {worst:?}");
    }

    // KOAN: (#1641) something drawn over a shader keeps what is drawn over
    // it in turn on top, where that sits clear of the shader: a menu over a
    // playing row keeps its text.
    #[test]
    fn what_covers_a_shader_keeps_its_own_order() {
        static PROGRAM: gpui::ShaderProgram = gpui::ShaderProgram {
            name: "koan-test-solid",
            msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    return float4(0.0, 1.0, 0.0, 1.0);
}
"#,
            wgsl: "",
            prepare: None,
        };
        let rect = |x: f32, y: f32, w: f32, h: f32| Bounds {
            origin: Point::new(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(w), ScaledPixels(h)),
        };
        let quad = |b: Bounds<ScaledPixels>, hue: f32| gpui::Quad {
            bounds: b,
            content_mask: ContentMask { bounds: b },
            background: gpui::hsla(hue, 0.8, 0.5, 1.0).into(),
            ..Default::default()
        };
        let mut scene = Scene::default();
        let shader = rect(4.0, 4.0, 16.0, 16.0);
        scene.insert_primitive(gpui::PaintShader {
            order: 0,
            bounds: shader,
            content_mask: ContentMask { bounds: shader },
            program: &PROGRAM,
            uniforms: Arc::new(gpui::ShaderUniforms::new()),
            image: None,
            group: None,
            group_inset: Default::default(),
        });
        // A menu over the shader, and its text over the menu, clear of it.
        scene.insert_primitive(quad(rect(12.0, 8.0, 40.0, 16.0), 0.1));
        scene.insert_primitive(quad(rect(32.0, 12.0, 12.0, 8.0), 0.6));
        scene.finish();
        let plan = scene.shader_layers.as_deref().expect("a plan");
        assert!(plan.layers[1].quads.len() == 2, "the text follows the menu");

        let viewport = size(DevicePixels(64), DevicePixels(32));
        let mut renderer = MetalRenderer::new_headless(Arc::new(Mutex::new(Default::default())));
        let whole = renderer.render_scene_to_image(&scene, viewport).unwrap();
        let composed = renderer
            .compose_scene_to_image(&scene, viewport, 1)
            .unwrap();
        assert_eq!(
            whole.get_pixel(36, 16),
            composed.get_pixel(36, 16),
            "the text is on top"
        );
        assert!(
            whole
                .as_raw()
                .iter()
                .zip(composed.as_raw())
                .all(|(a, b)| a.abs_diff(*b) <= 1)
        );
    }

    // KOAN: (#1641) GPUI's shader types, tested here: the vendored GPUI core
    // carries none of the assets its own tests read.
    mod shader {
        use gpui::{SHADER_UNIFORMS, ShaderInstance, ShaderUniforms, shader_epoch};
        #[test]
        fn uniforms_round_trip_and_liveness_is_separate() {
            let u = ShaderUniforms::new();
            u.set(3, 0.25);
            assert_eq!(u.get(3), 0.25);
            assert_eq!(u.snapshot()[3], 0.25);
            assert!(!u.is_live());
            u.set_live(true);
            assert!(u.is_live());
        }

        #[test]
        fn a_live_shader_presents_once_per_change_unless_continuous() {
            let u = ShaderUniforms::new();
            u.set(0, 1.0);
            assert!(u.take_present(), "a change presents, live or not");
            assert!(!u.take_present());
            u.set_live(true);
            u.set(0, 2.0);
            assert!(u.take_present());
            assert!(!u.take_present());
            u.set(0, 2.0);
            assert!(!u.take_present(), "the same value is no change");
            u.set_continuous(true);
            assert!(u.take_present());
            assert!(u.take_present());
        }

        #[test]
        fn the_instance_is_aligned_blocks() {
            assert_eq!(
                std::mem::size_of::<ShaderInstance>(),
                4 * (4 + 4 + SHADER_UNIFORMS + 4 + 4)
            );
        }

        #[test]
        fn time_counts_from_the_origin_and_moving_it_is_a_change() {
            let u = ShaderUniforms::new();
            u.set_live(true);
            let origin = shader_epoch() + std::time::Duration::from_secs(1_000_000);
            u.set_origin(origin);
            assert!(u.take_present());
            let t = u.time_at(origin + std::time::Duration::from_millis(1500));
            assert!((t - 1.5).abs() < 1e-6, "{t}");
            u.set_origin(origin);
            assert!(!u.take_present());
        }

        #[test]
        fn an_origin_before_the_epoch_counts_from_where_it_was() {
            let u = ShaderUniforms::new();
            let epoch = shader_epoch();
            let Some(before) = epoch.checked_sub(std::time::Duration::from_secs(5)) else {
                return;
            };
            u.set_origin(before);
            let t = u.time_at(epoch);
            assert!((t - 5.0).abs() < 1e-3, "{t}");
        }
    }
}

// KOAN: layer groups (#1859, #1878).
// KOAN: (#2034) a finished command buffer's time on the GPU, for
// `gpui::take_gpu_times`, while a measurement keeps them.
fn note_gpu_time(buffer: ptr::NonNull<AnyObject>) {
    if !gpui::gpu_timing_on() {
        return;
    }
    // SAFETY: the completed handler is handed the command buffer, an
    // MTLCommandBuffer, which answers both selectors.
    let (start, end): (f64, f64) = unsafe {
        let buffer = buffer.as_ref();
        (
            objc2::msg_send![buffer, GPUStartTime],
            objc2::msg_send![buffer, GPUEndTime],
        )
    };
    gpui::record_gpu_time(end - start);
}

#[cfg(test)]
mod layer_group_tests {
    use gpui::{
        Bounds, ContentMask, Path, Pixels, Quad, ScaledPixels, Scene, ShaderProgram,
        ShaderUniforms, Size, point, px,
    };

    static PROGRAM: ShaderProgram = ShaderProgram {
        name: "test",
        msl: "",
        wgsl: "",
        prepare: None,
    };

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: Size {
                width: ScaledPixels(w),
                height: ScaledPixels(h),
            },
        }
    }

    fn quad(at: Bounds<ScaledPixels>) -> Quad {
        Quad {
            bounds: at,
            content_mask: ContentMask { bounds: at },
            ..Default::default()
        }
    }

    fn shader(at: Bounds<ScaledPixels>) -> gpui::PaintShader {
        gpui::PaintShader {
            order: 0,
            bounds: at,
            content_mask: ContentMask { bounds: at },
            program: &PROGRAM,
            uniforms: std::sync::Arc::new(ShaderUniforms::default()),
            image: None,
            group: None,
            group_inset: Default::default(),
        }
    }

    /// What is painted in a group, shaders included, is the group's: drawn
    /// into its texture, so it moves and is clipped with the group. The
    /// window's scene has only the shader composing it.
    #[test]
    fn a_group_holds_what_is_painted_in_it_shaders_too() {
        let mut scene = Scene::default();
        scene.push_group();
        scene.insert_primitive(quad(bounds(110.0, 60.0, 20.0, 20.0)));
        scene.insert_primitive(shader(bounds(120.0, 70.0, 10.0, 10.0)));
        scene.pop_group(shader(bounds(100.0, 50.0, 200.0, 100.0)));
        scene.finish();
        assert!(scene.quads.is_empty());
        assert_eq!(scene.shaders.len(), 1);
        assert_eq!(scene.shaders[0].group, Some(0));
        let group = &scene.layer_groups[0];
        assert_eq!((group.quads.len(), group.shaders.len()), (1, 1));
    }

    fn stale(scene: &Scene, was_moving: &mut bool, written: bool, redrawn: &[bool]) -> bool {
        scene.layer_group_stale(0, std::time::Instant::now(), written, was_moving, redrawn)
    }

    /// A group is drawn again while a shader in it is live, once more after,
    /// and not at all while its shaders rest: a group that never goes away
    /// costs nothing between frames.
    #[test]
    fn a_group_is_drawn_again_while_its_shaders_move_and_once_after() {
        let mut scene = Scene::default();
        scene.push_group();
        let inner = shader(bounds(120.0, 70.0, 10.0, 10.0));
        let uniforms = inner.uniforms.clone();
        scene.insert_primitive(inner);
        scene.pop_group(shader(bounds(100.0, 50.0, 200.0, 100.0)));
        scene.finish();
        let mut was_moving = false;
        assert!(!stale(&scene, &mut was_moving, false, &[false]));
        uniforms.set_live(true);
        assert!(stale(&scene, &mut was_moving, false, &[false]));
        uniforms.set_live(false);
        assert!(
            stale(&scene, &mut was_moving, false, &[false]),
            "the landing"
        );
        assert!(!stale(&scene, &mut was_moving, false, &[false]));
    }

    /// A group composing one drawn again this present is drawn again too.
    #[test]
    fn a_group_follows_a_group_nested_in_it() {
        let mut scene = Scene::default();
        scene.push_group();
        scene.push_group();
        scene.insert_primitive(quad(bounds(10.0, 10.0, 5.0, 5.0)));
        scene.pop_group(shader(bounds(0.0, 0.0, 50.0, 50.0)));
        scene.pop_group(shader(bounds(0.0, 0.0, 100.0, 100.0)));
        scene.finish();
        let mut was_moving = false;
        assert!(!stale(&scene, &mut was_moving, false, &[false, false]));
        assert!(stale(&scene, &mut was_moving, false, &[false, true]));
    }

    /// A group with no paths draws into a texture its own size: its content
    /// moved to the texture's origin, the composing shader's tile the whole
    /// texture.
    #[test]
    fn a_group_without_paths_is_drawn_at_its_own_size() {
        let mut scene = Scene::default();
        scene.push_group();
        scene.insert_primitive(quad(bounds(110.0, 60.0, 20.0, 20.0)));
        scene.pop_group(shader(bounds(100.0, 50.0, 200.0, 100.0)));
        scene.finish();
        assert_eq!(
            scene.layer_group_frames[0],
            Some(bounds(100.0, 50.0, 200.0, 100.0))
        );
        let moved = &scene.layer_groups[0].quads[0];
        assert_eq!(moved.bounds, bounds(10.0, 10.0, 20.0, 20.0));
        assert_eq!(moved.content_mask.bounds, bounds(10.0, 10.0, 20.0, 20.0));
        let tile = gpui::ShaderInstance::of(&scene.shaders[0], std::time::Instant::now()).tile;
        assert_eq!(tile, [0.0, 0.0, 200.0, 100.0]);
    }

    /// Its texture is only the part of it inside its mask, the tile reaching
    /// back to the bounds' origin; one too large for a device takes the
    /// window's size.
    #[test]
    fn a_group_is_drawn_at_its_visible_size() {
        let mut scene = Scene::default();
        scene.push_group();
        scene.insert_primitive(quad(bounds(110.0, 60.0, 20.0, 20.0)));
        let mut compose = shader(bounds(100.0, -950.0, 200.0, 20_000.0));
        compose.content_mask = ContentMask {
            bounds: bounds(0.0, 0.0, 800.0, 600.0),
        };
        scene.pop_group(compose);
        scene.push_group();
        scene.insert_primitive(quad(bounds(110.0, 60.0, 20.0, 20.0)));
        let mut huge = shader(bounds(0.0, 0.0, 20_000.0, 100.0));
        huge.content_mask = ContentMask {
            bounds: bounds(0.0, 0.0, 10_000.0, 100.0),
        };
        scene.pop_group(huge);
        scene.finish();
        assert_eq!(
            scene.layer_group_frames[0],
            Some(bounds(100.0, 0.0, 200.0, 600.0))
        );
        assert_eq!(
            scene.layer_groups[0].quads[0].bounds,
            bounds(10.0, 60.0, 20.0, 20.0)
        );
        let tile = gpui::ShaderInstance::of(&scene.shaders[0], std::time::Instant::now()).tile;
        assert_eq!(tile, [0.0, -950.0, 200.0, 20_000.0]);
        assert_eq!(scene.layer_group_frames[1], None);
        let tile = gpui::ShaderInstance::of(&scene.shaders[1], std::time::Instant::now()).tile;
        assert_eq!(tile, [0.0, 0.0, 20_000.0, 100.0]);
    }

    /// A group at the window's size inside one at its own keeps its tile in
    /// window coordinates as the outer group's content moves.
    #[test]
    fn a_nested_group_with_a_path_keeps_its_tile() {
        let mut scene = Scene::default();
        scene.push_group();
        scene.push_group();
        let mut path: Path<Pixels> = Path::new(point(px(110.0), px(60.0)));
        path.line_to(point(px(150.0), px(60.0)));
        path.line_to(point(px(150.0), px(90.0)));
        let mut path = path.scale(1.0);
        path.content_mask = ContentMask {
            bounds: bounds(0.0, 0.0, 1000.0, 1000.0),
        };
        scene.insert_primitive(path);
        scene.pop_group(shader(bounds(105.0, 55.0, 60.0, 40.0)));
        scene.pop_group(shader(bounds(100.0, 50.0, 200.0, 100.0)));
        scene.finish();
        assert_eq!(scene.layer_group_frames[1], None);
        let inner = &scene.layer_groups[0].shaders[0];
        assert_eq!(
            inner.bounds.origin,
            point(ScaledPixels(5.0), ScaledPixels(5.0))
        );
        let tile = gpui::ShaderInstance::of(inner, std::time::Instant::now()).tile;
        assert_eq!(tile, [105.0, 55.0, 60.0, 40.0]);
    }

    /// One holding a path stays the window's size, its content where it was
    /// painted: paths are drawn through a texture the window's size.
    #[test]
    fn a_group_with_a_path_is_drawn_at_the_windows_size() {
        let mut scene = Scene::default();
        scene.push_group();
        let mut path: Path<Pixels> = Path::new(point(px(110.0), px(60.0)));
        path.line_to(point(px(150.0), px(60.0)));
        path.line_to(point(px(150.0), px(90.0)));
        let mut path = path.scale(1.0);
        path.content_mask = ContentMask {
            bounds: bounds(0.0, 0.0, 1000.0, 1000.0),
        };
        scene.insert_primitive(path);
        scene.pop_group(shader(bounds(100.0, 50.0, 200.0, 100.0)));
        scene.finish();
        assert_eq!(scene.layer_group_frames[0], None);
        let tile = gpui::ShaderInstance::of(&scene.shaders[0], std::time::Instant::now()).tile;
        assert_eq!(tile, [100.0, 50.0, 200.0, 100.0]);
    }

    /// A group inside another is composed in the outer group's scene, so the
    /// outer group carries it as it moves.
    #[test]
    fn a_nested_group_is_composed_inside_the_outer_one() {
        let mut scene = Scene::default();
        scene.push_group();
        scene.push_group();
        scene.insert_primitive(quad(bounds(10.0, 10.0, 5.0, 5.0)));
        scene.pop_group(shader(bounds(0.0, 0.0, 50.0, 50.0)));
        scene.pop_group(shader(bounds(0.0, 0.0, 100.0, 100.0)));
        scene.finish();
        assert_eq!(scene.layer_groups.len(), 2);
        assert_eq!(scene.shaders.len(), 1);
        assert_eq!(scene.shaders[0].group, Some(0));
        assert_eq!(scene.layer_groups[0].shaders[0].group, Some(1));
        assert_eq!(scene.layer_groups[1].quads.len(), 1);
    }

    /// A cached view's replay builds the same groups.
    #[test]
    fn a_replay_rebuilds_the_groups() {
        let mut first = Scene::default();
        first.push_group();
        first.insert_primitive(quad(bounds(110.0, 60.0, 20.0, 20.0)));
        first.insert_primitive(shader(bounds(120.0, 70.0, 10.0, 10.0)));
        first.pop_group(shader(bounds(100.0, 50.0, 200.0, 100.0)));
        let ops = first.len();
        let mut again = Scene::default();
        again.replay(0..ops, &first);
        again.finish();
        assert_eq!(again.layer_groups.len(), 1);
        assert_eq!(again.layer_groups[0].quads.len(), 1);
        assert_eq!(again.layer_groups[0].shaders.len(), 1);
        assert_eq!(again.shaders.len(), 1);
        assert_eq!(
            again.layer_group_frames[0],
            Some(bounds(100.0, 50.0, 200.0, 100.0))
        );
    }
}
