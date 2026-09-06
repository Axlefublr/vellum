mod geometry;
mod text;

use geometry::DrawCommand;
pub use geometry::{Geometry, LocalGeometry};
pub(crate) use text::{TextFont, init_text_font, layout_text, text_styles, with_text_context};
pub use text::{TextSpec, text_bounds, text_line_height, text_padding};

use kurbo::Affine;
use std::borrow::Cow;
use text::TextState;
use wayland_client::Proxy;
use wayland_client::protocol::wl_display::WlDisplay;
use wayland_client::protocol::wl_surface::WlSurface;
use wgpu::util::DeviceExt;

const PICKER_RENDER_SCALE: u32 = 2;

pub struct Viewport {
    pub origin: [f32; 2],
    pub scale: [f64; 2],
}

pub enum SceneItem<'a> {
    Geometry(Cow<'a, Geometry>),
    Text(TextSpec<'a>),
}

struct PickerTarget {
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    composite_buffer: wgpu::Buffer,
    size: [u16; 2],
    origin: [f32; 2],
}

fn composite_bytes(origin: [f32; 2]) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&origin[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&origin[1].to_ne_bytes());
    bytes
}

impl PickerTarget {
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: [u16; 2],
        origin: [f32; 2],
        layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("picker target"),
            size: wgpu::Extent3d {
                width: u32::from(size[0]),
                height: u32::from(size[1]),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let composite_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("picker composite origin"),
            contents: &composite_bytes(origin),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("picker composite"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: composite_buffer.as_entire_binding(),
                },
            ],
        });
        Self {
            view,
            bind_group,
            composite_buffer,
            size,
            origin,
        }
    }
}

pub struct WgpuState {
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    main_renderer: vello_hybrid::Renderer,
    main_resources: vello_hybrid::Resources,
    picker_renderer: vello_hybrid::Renderer,
    picker_resources: vello_hybrid::Resources,
    main_scene: vello_hybrid::Scene,
    picker_scene: vello_hybrid::Scene,
    texture_bindings: vello_hybrid::TextureBindings,
    picker_composite_pipeline: wgpu::RenderPipeline,
    picker_composite_layout: wgpu::BindGroupLayout,
    picker_target: Option<PickerTarget>,
    text: TextState,
}

pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl GpuContext {
    pub fn new(
        display: &WlDisplay,
        surface: &WlSurface,
        width: u32,
        height: u32,
    ) -> Result<(Self, WgpuState), String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = create_surface(&instance, display, surface)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|error| format!("could not select a Vulkan adapter: {error}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|error| format!("could not create the GPU device: {error}"))?;
        let gpu = Self {
            instance,
            adapter,
            device,
            queue,
        };
        let wgpu = WgpuState::new(&gpu, surface, width, height)?;
        Ok((gpu, wgpu))
    }

    pub fn create_surface(
        &self,
        display: &WlDisplay,
        surface: &WlSurface,
    ) -> Result<wgpu::Surface<'static>, String> {
        create_surface(&self.instance, display, surface)
    }
}

fn create_surface(
    instance: &wgpu::Instance,
    display: &WlDisplay,
    surface: &WlSurface,
) -> Result<wgpu::Surface<'static>, String> {
    let raw_display_handle =
        wgpu::rwh::RawDisplayHandle::Wayland(wgpu::rwh::WaylandDisplayHandle::new(
            std::ptr::NonNull::new(display.id().as_ptr() as *mut _).unwrap(),
        ));
    let raw_window_handle =
        wgpu::rwh::RawWindowHandle::Wayland(wgpu::rwh::WaylandWindowHandle::new(
            std::ptr::NonNull::new(surface.id().as_ptr() as *mut _).unwrap(),
        ));
    unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(raw_display_handle),
            raw_window_handle,
        })
    }
    .map_err(|error| format!("could not create the Wayland GPU surface: {error}"))
}

impl WgpuState {
    pub fn new(
        gpu: &GpuContext,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        checked_target_size(&gpu.device, [width, height], "annotation")?;
        let capabilities = surface.get_capabilities(&gpu.adapter);
        let format = capabilities
            .formats
            .iter()
            .find(|format| format.is_srgb())
            .copied()
            .or_else(|| capabilities.formats.first().copied())
            .ok_or("GPU adapter does not support the Wayland surface")?;
        let alpha_mode = capabilities
            .alpha_modes
            .iter()
            .find(|mode| matches!(mode, wgpu::CompositeAlphaMode::PreMultiplied))
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let device = gpu.device.clone();
        let queue = gpu.queue.clone();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let picker_composite_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("picker composite"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let picker_composite_shader =
            device.create_shader_module(wgpu::include_wgsl!("picker_composite.wgsl"));
        let picker_composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("picker composite pipeline"),
                bind_group_layouts: &[Some(&picker_composite_layout)],
                immediate_size: 0,
            });
        let picker_composite_pipeline = create_picker_composite_pipeline(
            &device,
            &picker_composite_pipeline_layout,
            &picker_composite_shader,
            format,
        );
        let target_config = vello_hybrid::RenderTargetConfig {
            format,
            width: 1,
            height: 1,
        };
        let mut main_settings = vello_hybrid::RenderSettings::default();
        // Text is the main scene's only atlas user; 1024px avoids a 64 MiB first-use allocation.
        main_settings.memory_settings.image_atlas_config.atlas_size = (1024, 1024);
        let (main_renderer, main_resources) =
            vello_hybrid::Renderer::new_with(&device, &target_config, main_settings);
        let (picker_renderer, picker_resources) =
            vello_hybrid::Renderer::new(&device, &target_config);

        Ok(Self {
            surface,
            surface_config,
            device,
            queue,
            main_renderer,
            main_resources,
            picker_renderer,
            picker_resources,
            main_scene: vello_hybrid::Scene::new(1, 1),
            picker_scene: vello_hybrid::Scene::new(1, 1),
            texture_bindings: vello_hybrid::TextureBindings::new(),
            picker_composite_pipeline,
            picker_composite_layout,
            picker_target: None,
            text: TextState::default(),
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0
            || height == 0
            || (width == self.surface_config.width && height == self.surface_config.height)
        {
            return Ok(());
        }
        checked_target_size(&self.device, [width, height], "annotation")?;
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        Ok(())
    }

    fn composite_picker(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        source: &PickerTarget,
        viewport: [f32; 4],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite picker"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.picker_composite_pipeline);
        let [x, y, width, height] = viewport;
        pass.set_viewport(x, y, width, height, 0.0, 1.0);
        pass.set_bind_group(0, &source.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    pub fn render(
        &mut self,
        items: &[SceneItem<'_>],
        previews: &[Geometry],
        picker: Option<&LocalGeometry>,
        viewport: Viewport,
        active_text: Option<(u64, &parley::Layout<()>)>,
        before_present: impl FnOnce(),
    ) -> Result<bool, String> {
        let Viewport {
            origin: viewport_origin,
            scale,
        } = viewport;
        let mut status = self.surface.get_current_texture();
        if matches!(
            status,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost
        ) {
            self.surface.configure(&self.device, &self.surface_config);
            status = self.surface.get_current_texture();
        }
        let output = match status {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Lost => return Ok(false),
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface acquisition validation error".into());
            }
        };

        let main_size = checked_target_size(
            &self.device,
            [self.surface_config.width, self.surface_config.height],
            "annotation",
        )?;
        self.main_scene.reset_and_resize(main_size[0], main_size[1]);
        self.main_scene.set_transform(
            Affine::scale_non_uniform(scale[0], scale[1])
                * Affine::translate((
                    -f64::from(viewport_origin[0]),
                    -f64::from(viewport_origin[1]),
                )),
        );
        let target_is_srgb = self.surface_config.format.is_srgb();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let mut target = text::TextTarget {
            scene: &mut self.main_scene,
            resources: &mut self.main_resources,
            renderer: &mut self.main_renderer,
            device: &self.device,
            queue: &self.queue,
            encoder: &mut encoder,
            is_srgb: target_is_srgb,
        };
        for item in items {
            match item {
                SceneItem::Geometry(geometry) => {
                    replay_geometry(target.scene, geometry, target_is_srgb)
                }
                SceneItem::Text(spec) => self.text.append_to_scene(&mut target, spec, active_text),
            }
        }
        self.text.finish_frame(&mut target);
        for geometry in previews {
            replay_geometry(&mut self.main_scene, geometry, target_is_srgb);
        }

        let picker_origin = picker.map_or([0.0; 2], |picker| {
            [
                (f64::from(picker.origin[0] - viewport_origin[0]) * scale[0]) as f32,
                (f64::from(picker.origin[1] - viewport_origin[1]) * scale[1]) as f32,
            ]
        });
        let picker_size = if let Some(picker) = picker {
            let size = [
                (f64::from(picker.size[0]) * scale[0]).ceil() as u32,
                (f64::from(picker.size[1]) * scale[1]).ceil() as u32,
            ];
            let scene_size = checked_picker_scene_size(&self.device, size)?;
            if self
                .picker_target
                .as_ref()
                .is_none_or(|target| target.size != scene_size)
            {
                self.picker_target = Some(PickerTarget::new(
                    &self.device,
                    self.surface_config.format,
                    scene_size,
                    picker_origin,
                    &self.picker_composite_layout,
                ));
            }
            let target = self.picker_target.as_mut().unwrap();
            if target.origin != picker_origin {
                self.queue.write_buffer(
                    &target.composite_buffer,
                    0,
                    &composite_bytes(picker_origin),
                );
                target.origin = picker_origin;
            }
            Some(scene_size)
        } else {
            None
        };

        if let (Some(picker), Some(size)) = (picker, picker_size) {
            self.picker_scene.reset_and_resize(size[0], size[1]);
            self.picker_scene.set_transform(
                Affine::scale(f64::from(PICKER_RENDER_SCALE))
                    * Affine::scale_non_uniform(scale[0], scale[1]),
            );
            replay_geometry(&mut self.picker_scene, &picker.geometry, target_is_srgb);
        }

        let swapchain_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let render_size = vello_hybrid::RenderSize {
            width: u32::from(main_size[0]),
            height: u32::from(main_size[1]),
        };
        self.main_renderer
            .render(
                &self.main_scene,
                &mut self.main_resources,
                &self.device,
                &self.queue,
                &mut encoder,
                &render_size,
                &swapchain_view,
                &self.texture_bindings,
            )
            .map_err(|error| format!("Vello annotation render failed: {error}"))?;
        if let (Some(picker), Some(size)) = (picker, picker_size) {
            let render_size = vello_hybrid::RenderSize {
                width: u32::from(size[0]),
                height: u32::from(size[1]),
            };
            self.picker_renderer
                .render(
                    &self.picker_scene,
                    &mut self.picker_resources,
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    &render_size,
                    &self.picker_target.as_ref().unwrap().view,
                    &self.texture_bindings,
                )
                .map_err(|error| format!("Vello picker render failed: {error}"))?;
            let left = picker_origin[0].max(0.0);
            let top = picker_origin[1].max(0.0);
            let right = (picker_origin[0] + picker.size[0] as f32 * scale[0] as f32)
                .min(self.surface_config.width as f32);
            let bottom = (picker_origin[1] + picker.size[1] as f32 * scale[1] as f32)
                .min(self.surface_config.height as f32);
            if right > left && bottom > top {
                self.composite_picker(
                    &mut encoder,
                    &swapchain_view,
                    self.picker_target.as_ref().unwrap(),
                    [left, top, right - left, bottom - top],
                );
            }
        }
        self.queue.submit(Some(encoder.finish()));
        before_present();
        output.present();
        Ok(true)
    }

    pub fn release_picker_target(&mut self) {
        self.picker_target = None;
    }
}

fn checked_target_size(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
) -> Result<[u16; 2], String> {
    let limit = device
        .limits()
        .max_texture_dimension_2d
        .min(u32::from(u16::MAX));
    if size
        .iter()
        .any(|dimension| *dimension == 0 || *dimension > limit)
    {
        return Err(format!(
            "{label} target {}x{} must have dimensions in 1..={limit}",
            size[0], size[1]
        ));
    }
    Ok([size[0] as u16, size[1] as u16])
}

fn checked_picker_scene_size(device: &wgpu::Device, size: [u32; 2]) -> Result<[u16; 2], String> {
    let scaled = size.map(|dimension| dimension.checked_mul(PICKER_RENDER_SCALE));
    let [Some(width), Some(height)] = scaled else {
        return Err(format!(
            "picker target {}x{} overflows at {PICKER_RENDER_SCALE}x scale",
            size[0], size[1]
        ));
    };
    checked_target_size(device, [width, height], "picker")
}

fn replay_geometry(scene: &mut vello_hybrid::Scene, geometry: &Geometry, target_is_srgb: bool) {
    for command in &geometry.commands {
        match command {
            DrawCommand::Fill {
                path,
                fill_rule,
                color,
            } => {
                scene.set_paint(vello_color(*color, target_is_srgb));
                scene.set_fill_rule(*fill_rule);
                scene.fill_path(path);
            }
            DrawCommand::Stroke {
                path,
                stroke,
                color,
            } => {
                scene.set_paint(vello_color(*color, target_is_srgb));
                scene.set_stroke(stroke.clone());
                scene.stroke_path(path);
            }
        }
    }
}

fn srgb_to_linear(component: f32) -> f32 {
    if component <= 0.04045 {
        component / 12.92
    } else {
        ((component + 0.055) / 1.055).powf(2.4)
    }
}

fn vello_color([red, green, blue, alpha]: [f32; 4], target_is_srgb: bool) -> peniko::Color {
    let convert = |component| {
        if target_is_srgb {
            srgb_to_linear(component)
        } else {
            component
        }
    };
    peniko::Color::new([convert(red), convert(green), convert(blue), alpha])
}

fn create_picker_composite_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("picker composite pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[("render_scale", PICKER_RENDER_SCALE as f64)],
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
