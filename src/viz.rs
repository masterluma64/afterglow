//! GPU music visualizer drawn through egui's wgpu renderer.
//!
//! Two floating-point textures are used as a feedback loop: each frame the
//! warp shader reads the previous frame and writes the next one, and the
//! composite shader shows the result. See `viz.wgsl`.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};

use crate::audio::{BANDS, Features};

const FEEDBACK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[repr(C)]
#[derive(Clone, Copy)]
struct Uniforms {
    res: [f32; 2],
    time: f32,
    drive: f32,
    bass: f32,
    mid: f32,
    treble: f32,
    beat: f32,
    level: f32,
    preset: f32,
    flash: f32,
    aspect: f32,
    spec: [[f32; 4]; BANDS / 4],
    wave: [[f32; 4]; BANDS / 4],
}

impl Uniforms {
    fn bytes(&self) -> &[u8] {
        // Plain f32 data with no padding (checked by the shader layout).
        unsafe { std::slice::from_raw_parts(self as *const Self as *const u8, std::mem::size_of::<Self>()) }
    }
}

struct Targets {
    size: [u32; 2],
    _textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    /// `groups[i]` samples `views[i]`.
    groups: [wgpu::BindGroup; 2],
}

pub struct VizResources {
    warp: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    targets: Option<Targets>,
    /// Index of the texture holding the latest frame.
    current: usize,
}

/// Creates the GPU resources and registers them with egui's renderer.
/// Returns false when eframe isn't running on wgpu.
pub fn install(cc: &eframe::CreationContext) -> bool {
    let Some(state) = cc.wgpu_render_state.as_ref() else {
        return false;
    };
    let resources = VizResources::new(&state.device, state.target_format);
    state.renderer.write().callback_resources.insert(resources);
    true
}

impl VizResources {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viz"),
            source: wgpu::ShaderSource::Wgsl(include_str!("viz.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viz"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viz"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let warp = pipeline("fs_warp", FEEDBACK_FORMAT);
        let composite = pipeline("fs_composite", target_format);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viz"),
            address_mode_u: wgpu::AddressMode::MirrorRepeat,
            address_mode_v: wgpu::AddressMode::MirrorRepeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viz"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        VizResources {
            warp,
            composite,
            layout,
            sampler,
            uniforms,
            targets: None,
            current: 0,
        }
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let make = || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("viz feedback"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FEEDBACK_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        };
        let textures = [make(), make()];
        let views = textures.each_ref().map(|t| t.create_view(&Default::default()));
        let groups = views.each_ref().map(|view| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("viz"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniforms.as_entire_binding(),
                    },
                ],
            })
        });
        self.targets = Some(Targets {
            size,
            _textures: textures,
            views,
            groups,
        });
        self.current = 0;
    }

    /// Runs one feedback step: warps the latest frame into the other texture.
    fn advance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniforms: &Uniforms,
        size: [u32; 2],
    ) {
        self.ensure_targets(device, size);
        queue.write_buffer(&self.uniforms, 0, uniforms.bytes());
        let targets = self.targets.as_ref().unwrap();
        let next = 1 - self.current;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viz warp"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.views[next],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.warp);
            pass.set_bind_group(0, &targets.groups[self.current], &[]);
            pass.draw(0..3, 0..1);
        }
        self.current = next;
    }

    /// Draws the latest frame (tone mapped) into the current pass.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(targets) = &self.targets else { return };
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, &targets.groups[self.current], &[]);
        pass.draw(0..3, 0..1);
    }
}

/// One frame of the visualizer, added to the painter for a rect.
struct VizCallback {
    uniforms: Uniforms,
    size: [u32; 2],
}

impl egui_wgpu::CallbackTrait for VizCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get_mut::<VizResources>() {
            res.advance(device, queue, encoder, &self.uniforms, self.size);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(res) = resources.get::<VizResources>() {
            res.draw(pass);
        }
    }
}

/// Paints the visualizer into `rect` with the given preset (0..PRESETS).
impl Uniforms {
    fn new(features: &Features, preset: usize, time: f32, size: [u32; 2]) -> Self {
        let pack = |v: &[f32; BANDS]| {
            let mut out = [[0.0; 4]; BANDS / 4];
            for (i, x) in v.iter().enumerate() {
                out[i / 4][i % 4] = *x;
            }
            out
        };
        Self {
            res: [size[0] as f32, size[1] as f32],
            time,
            drive: features.drive,
            bass: features.bass,
            mid: features.mid,
            treble: features.treble,
            beat: features.beat,
            level: features.level,
            preset: preset as f32,
            flash: features.beat,
            aspect: size[0] as f32 / size[1].max(1) as f32,
            spec: pack(&features.spectrum),
            wave: pack(&features.wave),
        }
    }
}

/// Paints the visualizer into `rect` with the given preset (index into
/// `Visualizer::PRESETS`).
pub fn paint(ui: &egui::Ui, rect: egui::Rect, features: &Features, preset: usize, time: f32) {
    let ppp = ui.ctx().pixels_per_point();
    let size = [
        (rect.width() * ppp).round().max(1.0) as u32,
        (rect.height() * ppp).round().max(1.0) as u32,
    ];
    let uniforms = Uniforms::new(features, preset, time, size);
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        rect,
        VizCallback { uniforms, size },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::Visualizer;

    fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
    }

    /// Synthetic music: a moving spectrum, a sine-ish waveform and beats.
    fn features(frame: usize) -> Features {
        let t = frame as f32 / 60.0;
        let mut f = crate::audio::Analyzer::default().tick(0.0);
        for i in 0..BANDS {
            let x = i as f32 / BANDS as f32;
            f.spectrum[i] = ((1.0 - x) * (0.6 + 0.4 * (t * 3.0 + x * 9.0).sin())).max(0.0);
            f.wave[i] = (x * 25.0 + t * 7.0).sin() * 0.8;
        }
        (f.bass, f.mid, f.treble, f.level) = (0.7, 0.5, 0.3, 0.5);
        f.beat = if frame % 30 < 5 { 1.0 } else { 0.0 };
        f.drive = t * 1.2;
        f
    }

    /// Every preset compiles, runs, and produces a bright, colorful image.
    #[test]
    fn presets_render_colorful_frames() {
        let Some((device, queue)) = gpu() else {
            eprintln!("no GPU adapter available; skipping");
            return;
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut res = VizResources::new(&device, format);
        let size = [320u32, 180];
        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let out_view = out.create_view(&Default::default());
        let row = size[0] * 4;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * size[1]) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut failures = Vec::new();
        for (preset, vis) in Visualizer::PRESETS.iter().enumerate() {
            res.targets = None;
            for frame in 0..120 {
                let mut encoder = device.create_command_encoder(&Default::default());
                let uniforms = Uniforms::new(&features(frame), preset, frame as f32 / 60.0, size);
                res.advance(&device, &queue, &mut encoder, &uniforms, size);
                queue.submit([encoder.finish()]);
            }
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &out_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                res.draw(&mut pass);
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &out,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(size[1]),
                    },
                },
                wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);
            readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let pixels = readback.slice(..).get_mapped_range().unwrap().to_vec();
            readback.unmap();

            if let Some(dir) = std::env::var_os("VIZ_DUMP") {
                let img = image::RgbaImage::from_raw(size[0], size[1], pixels.clone()).unwrap();
                img.save(std::path::Path::new(&dir).join(format!("{}.png", vis.label())))
                    .unwrap();
            }
            let n = (size[0] * size[1]) as f32;
            let lum: f32 = pixels
                .chunks(4)
                .map(|p| (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0)
                .sum::<f32>()
                / n;
            let lit = pixels.chunks(4).filter(|p| p[0].max(p[1]).max(p[2]) > 60).count() as f32 / n;
            let colorful = pixels
                .chunks(4)
                .filter(|p| p[0].max(p[1]).max(p[2]) as i32 - p[0].min(p[1]).min(p[2]) as i32 > 40)
                .count();
            println!(
                "{:>10}: mean {lum:5.1}, lit {:4.1}%, colorful px {colorful}",
                vis.label(),
                lit * 100.0
            );
            let dark = pixels.chunks(4).filter(|p| p[0].max(p[1]).max(p[2]) < 40).count() as f32 / n;
            if lit < 0.02 {
                failures.push(format!("{} is nearly black", vis.label()));
            }
            if lum > 140.0 || dark < 0.1 {
                failures.push(format!("{} is washed out (mean {lum:.0}, dark {:.0}%)", vis.label(), dark * 100.0));
            }
            if colorful < 500 {
                failures.push(format!("{} has no color", vis.label()));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
