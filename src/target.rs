//! Where the map draws, and how it reaches the window.
//!
//! mln-rs's own window backend (`mln_webgpu::SurfaceBackend`) draws the map
//! straight into the window. On the globe the map clears to transparent and
//! draws nothing around the planet but its atmosphere, so this target has the
//! map draw into a texture instead, and `present` draws a field of stars into
//! the window with the map's frame over it (stars.wgsl).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use mln_rs::mln_gfx::context::Context;
use mln_rs::mln_gfx::renderable::{Renderable, RenderableBase};
use mln_rs::mln_gfx::renderer_backend::{self, ContextMode, RendererBackendBase};
use mln_rs::mln_gfx::shader_registry::ShaderRegistry;
use mln_rs::mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_rs::mln_util::image::PremultipliedImage;
use mln_rs::mln_util::size::Size;
use mln_rs::mln_webgpu::RendererBackend;
use mln_rs::mln_webgpu::renderable_resource::{RenderableResource, RenderableResourceBox};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

/// The formats the map draws in (those of mln-rs's headless backend).
const MAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// The map's textures, at the window's size, and the bind group that shows
/// its frame to the window pass.
struct MapTextures {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: RefCell<wgpu::SurfaceConfiguration>,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    map: RefCell<MapTextures>,
}

impl Gpu {
    fn map_textures(&self, size: Size) -> MapTextures {
        map_textures(&self.device, &self.layout, &self.params, size)
    }
}

fn extent(size: Size) -> wgpu::Extent3d {
    wgpu::Extent3d { width: size.width.max(1), height: size.height.max(1), depth_or_array_layers: 1 }
}

fn map_textures(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    size: Size,
) -> MapTextures {
    let view = |label, format, usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent(size),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    };
    let color = view("map", MAP_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let depth = view("map depth and stencil", DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("stars"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&color) },
        ],
    });
    MapTextures { color, depth, bind_group }
}

/// The map's render target: its texture, as the renderable resource the
/// map's render passes draw into.
struct MapTarget(Rc<Gpu>);

impl RenderableResource for MapTarget {
    fn get_color_texture_view(&self) -> Option<wgpu::TextureView> {
        Some(self.0.map.borrow().color.clone())
    }
    fn get_color_texture_format(&self) -> Option<wgpu::TextureFormat> {
        Some(MAP_FORMAT)
    }
    fn get_depth_stencil_texture_view(&self) -> Option<wgpu::TextureView> {
        Some(self.0.map.borrow().depth.clone())
    }
    fn get_depth_stencil_texture_format(&self) -> Option<wgpu::TextureFormat> {
        Some(DEPTH_FORMAT)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The renderer backend for a window: the map draws into its texture,
/// `present` shows it over the stars.
pub struct WindowTarget {
    backend: RendererBackend,
    renderable: RenderableBase,
    gpu: Rc<Gpu>,
    pixel_ratio: f32,
    started: Instant,
}

impl WindowTarget {
    pub fn new(event_loop: &ActiveEventLoop, window: Arc<Window>) -> Result<WindowTarget, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
            event_loop.owned_display_handle(),
        )));
        let surface =
            instance.create_surface(window.clone()).map_err(|e| format!("Could not draw into the window: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("No WebGPU adapter: {e}"))?;
        // The WebGPU default limits, lowered to what the adapter has, as
        // mln-rs's backends ask for them.
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mln-rs globe"),
            required_limits: wgpu::Limits::default().or_worse_values_from(&adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| format!("No WebGPU device: {e}"))?;
        device.on_uncaptured_error(Arc::new(|error: wgpu::Error| eprintln!("WebGPU error: {error}")));

        // An 8-bit format without sRGB encoding, as mln-rs's window backend
        // picks: the map's colors are sRGB already.
        let capabilities = surface.get_capabilities(&adapter);
        let format = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm]
            .into_iter()
            .find(|f| capabilities.formats.contains(f))
            .or_else(|| capabilities.formats.first().copied())
            .ok_or("The window has no texture formats")?;
        let physical = window.inner_size();
        let size = Size::new(physical.width.max(1), physical.height.max(1));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stars"),
            source: wgpu::ShaderSource::Wgsl(include_str!("stars.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stars"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("stars"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stars"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stars"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let map = map_textures(&device, &layout, &params, size);

        let backend = RendererBackend::new(ContextMode::Unique);
        backend.set_color_format(Some(MAP_FORMAT));
        backend.set_depth_stencil_format(Some(DEPTH_FORMAT));
        let backend_impl = backend.backend_impl();
        backend_impl.set_instance(Some(instance));
        backend_impl.set_adapter(Some(adapter));
        backend_impl.set_device(Some(device.clone()));
        backend_impl.set_queue(Some(queue.clone()));

        let gpu = Rc::new(Gpu {
            device,
            queue,
            surface,
            config: RefCell::new(config),
            pipeline,
            layout,
            params,
            map: RefCell::new(map),
        });
        let resource = RenderableResourceBox::new_rc(MapTarget(gpu.clone()));
        Ok(WindowTarget {
            backend,
            renderable: RenderableBase::new(size, Some(resource)),
            gpu,
            pixel_ratio: window.scale_factor() as f32,
            started: Instant::now(),
        })
    }

    /// Follows the window to `size`, in physical pixels.
    pub fn resize(&self, size: Size) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        {
            let mut config = self.gpu.config.borrow_mut();
            config.width = size.width;
            config.height = size.height;
            self.gpu.surface.configure(&self.gpu.device, &config);
        }
        *self.gpu.map.borrow_mut() = self.gpu.map_textures(size);
        self.set_renderable_size(size);
    }

    /// Shows the stars and the map's latest frame in the window.
    pub fn present(&self) {
        let frame = match self.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.gpu.surface.configure(&self.gpu.device, &self.gpu.config.borrow());
                return;
            }
            // Hidden or busy: skip this frame.
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return,
        };
        self.draw(&frame.texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.gpu.queue.present(frame);
    }

    /// What `present` shows, as an RGBA image.
    pub fn screenshot(&self) -> Result<PremultipliedImage, String> {
        let gpu = &self.gpu;
        let size = self.get_size();
        let format = gpu.config.borrow().format;
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot"),
            size: extent(size),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.draw(&texture.create_view(&wgpu::TextureViewDescriptor::default()));

        let bytes_per_row =
            (size.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot"),
            size: u64::from(bytes_per_row) * u64::from(size.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder =
            gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("screenshot") });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(size.height),
                },
            },
            extent(size),
        );
        gpu.queue.submit([encoder.finish()]);
        let (sender, mapped) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("Screenshot: {e}"))?;
        mapped.recv().map_err(|e| format!("Screenshot: {e}"))?.map_err(|e| format!("Screenshot: {e}"))?;

        let bgra = matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut image = PremultipliedImage::new(size);
        {
            let data = buffer.get_mapped_range(..).map_err(|e| format!("Screenshot: {e}"))?;
            let row = size.width as usize * 4;
            for y in 0..size.height as usize {
                let pixels = &mut image.data[y * row..][..row];
                pixels.copy_from_slice(&data[y * bytes_per_row as usize..][..row]);
                if bgra {
                    for pixel in pixels.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                }
            }
        }
        buffer.unmap();
        Ok(image)
    }

    /// Draws the stars and the map's frame over them into `target`.
    fn draw(&self, target: &wgpu::TextureView) {
        let gpu = &self.gpu;
        let size = self.get_size();
        let mut params = [0u8; 16];
        let values = [size.width as f32, size.height as f32, self.started.elapsed().as_secs_f32(), self.pixel_ratio];
        for (bytes, value) in params.as_chunks_mut::<4>().0.iter_mut().zip(values) {
            *bytes = value.to_ne_bytes();
        }
        gpu.queue.write_buffer(&gpu.params, 0, &params);

        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("stars") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stars"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &gpu.map.borrow().bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue.submit([encoder.finish()]);
    }
}

impl Renderable for WindowTarget {
    fn renderable_base(&self) -> &RenderableBase {
        &self.renderable
    }
}

impl renderer_backend::RendererBackend for WindowTarget {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.backend.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_default_renderable(&self) -> &dyn Renderable {
        self
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        self.backend.init_shaders(registry, parameters);
    }

    fn create_context(&self) -> Rc<dyn Context> {
        self.backend.create_context()
    }

    fn activate(&self) {}

    fn deactivate(&self) {}
}
