//! Native wgpu port of the browser's neural mega-kernel.
//!
//! wgpu lowers this compute path to Vulkan on supported Linux/Windows systems
//! (and to Metal on macOS). The packed model is shared byte-for-byte with the
//! WebGPU implementation so native and browser output use identical weights.
//! We request shader-f16 and use the same packed weights, FP16 activations,
//! workgroup tiling, and dispatch order as `web/nn-upscale.js`.

use std::num::NonZeroU64;

use pixels::wgpu;
use pixels::wgpu::util::DeviceExt;

pub(crate) const INPUT_W: u32 = 320;
pub(crate) const INPUT_H: u32 = 200;
const SCALE: u32 = 4;
const OUTPUT_W: u32 = INPUT_W * SCALE;
const OUTPUT_H: u32 = INPUT_H * SCALE;
/// One row of the output texture. 5120 is already a multiple of wgpu's
/// 256-byte copy alignment, so texture-to-buffer copies need no padding.
const OUTPUT_ROW_BYTES: u64 = OUTPUT_W as u64 * 4;
const VEC4_F16_BYTES: u64 = 8;
const ACTIVATION_VECS_PER_PIXEL: u64 = 16;
const ACTIVATION_BYTES: u64 =
    INPUT_W as u64 * INPUT_H as u64 * ACTIVATION_VECS_PER_PIXEL * VEC4_F16_BYTES;
const UNIFORM_STRIDE: u64 = 256;
const MODEL_BYTES: &[u8] = include_bytes!("../web/models/realesr-animevideov3-fp16.mega.bin");

#[derive(Clone, Copy)]
struct LayerMeta {
    mat_base: u32,
    bias_base: u32,
    slope_base: u32,
    output_vecs: u32,
    input_vecs: u32,
}

const LAYERS: [LayerMeta; 18] = [
    LayerMeta {
        mat_base: 0,
        bias_base: 154944,
        slope_base: 154960,
        output_vecs: 16,
        input_vecs: 1,
    },
    LayerMeta {
        mat_base: 144,
        bias_base: 154976,
        slope_base: 154992,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 2448,
        bias_base: 155008,
        slope_base: 155024,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 4752,
        bias_base: 155040,
        slope_base: 155056,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 7056,
        bias_base: 155072,
        slope_base: 155088,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 9360,
        bias_base: 155104,
        slope_base: 155120,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 11664,
        bias_base: 155136,
        slope_base: 155152,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 13968,
        bias_base: 155168,
        slope_base: 155184,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 16272,
        bias_base: 155200,
        slope_base: 155216,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 18576,
        bias_base: 155232,
        slope_base: 155248,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 20880,
        bias_base: 155264,
        slope_base: 155280,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 23184,
        bias_base: 155296,
        slope_base: 155312,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 25488,
        bias_base: 155328,
        slope_base: 155344,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 27792,
        bias_base: 155360,
        slope_base: 155376,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 30096,
        bias_base: 155392,
        slope_base: 155408,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 32400,
        bias_base: 155424,
        slope_base: 155440,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 34704,
        bias_base: 155456,
        slope_base: 155472,
        output_vecs: 16,
        input_vecs: 16,
    },
    LayerMeta {
        mat_base: 37008,
        bias_base: 155488,
        slope_base: 0,
        output_vecs: 12,
        input_vecs: 16,
    },
];

pub(crate) struct NativeUpscaler {
    _uniform: wgpu::Buffer,
    input: wgpu::Texture,
    output: wgpu::Texture,
    first_bind_group: wgpu::BindGroup,
    mid_ab_bind_group: wgpu::BindGroup,
    mid_ba_bind_group: wgpu::BindGroup,
    last_bind_group: wgpu::BindGroup,
    blit_bind_group: wgpu::BindGroup,
    first_pipeline: wgpu::ComputePipeline,
    mid_pipeline: wgpu::ComputePipeline,
    mid_pixels_per_workgroup: u32,
    last_pipeline: wgpu::ComputePipeline,
    blit_pipeline: wgpu::RenderPipeline,
}

impl NativeUpscaler {
    pub(crate) fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        assert_eq!(MODEL_BYTES.len(), 1_244_000, "unexpected neural model size");
        validate_model_metadata();

        let model = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native upscale model"),
            contents: MODEL_BYTES,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let uniform_data = layer_uniform_data();
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native upscale layer uniforms"),
            contents: &uniform_data,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let activation = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: ACTIVATION_BYTES,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        let activation_a = activation("native upscale activation A");
        let activation_b = activation("native upscale activation B");
        let input = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native upscale input"),
            size: wgpu::Extent3d {
                width: INPUT_W,
                height: INPUT_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native upscale output"),
            size: wgpu::Extent3d {
                width: OUTPUT_W,
                height: OUTPUT_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let first_layout = first_bind_group_layout(device);
        let mid_layout = mid_bind_group_layout(device);
        let last_layout = last_bind_group_layout(device);
        let uniform_binding = || wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &uniform,
                offset: 0,
                size: NonZeroU64::new(UNIFORM_STRIDE),
            }),
        };
        let model_mats = || wgpu::BindGroupEntry {
            binding: 1,
            resource: model.as_entire_binding(),
        };
        let model_vecs = || wgpu::BindGroupEntry {
            binding: 2,
            resource: model.as_entire_binding(),
        };
        let first_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native upscale first bind group"),
            layout: &first_layout,
            entries: &[
                uniform_binding(),
                model_mats(),
                model_vecs(),
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: activation_a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
            ],
        });
        let mid_group = |label, src: &wgpu::Buffer, dst: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &mid_layout,
                entries: &[
                    uniform_binding(),
                    model_mats(),
                    model_vecs(),
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: src.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: dst.as_entire_binding(),
                    },
                ],
            })
        };
        let mid_ab_bind_group =
            mid_group("native upscale mid A to B", &activation_a, &activation_b);
        let mid_ba_bind_group =
            mid_group("native upscale mid B to A", &activation_b, &activation_a);
        let last_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native upscale last bind group"),
            layout: &last_layout,
            entries: &[
                uniform_binding(),
                model_mats(),
                model_vecs(),
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: activation_a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
            ],
        });

        let base_shader = trusted_shader(
            device,
            "native upscale first/last shader",
            include_str!("shaders/native_upscale.wgsl"),
        );
        let fast_mid = device.limits().max_compute_workgroup_storage_size >= 23_040;
        let mid_source = if fast_mid {
            include_str!("shaders/native_upscale_mid_fast.wgsl")
        } else {
            include_str!("shaders/native_upscale_mid.wgsl")
        };
        let mid_shader = trusted_shader(device, "native upscale middle shader", mid_source);
        let first_pipeline = compute_pipeline(device, &first_layout, &base_shader, "conv_first");
        let mid_pipeline = compute_pipeline(device, &mid_layout, &mid_shader, "conv_mid");
        let last_pipeline = compute_pipeline(device, &last_layout, &base_shader, "conv_last");

        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native upscale blit layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native upscale blit sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native upscale blit bind group"),
            layout: &blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native upscale blit shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("shaders/native_upscale_blit.wgsl").into(),
            ),
        });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native upscale blit pipeline layout"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native upscale blit pipeline"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            _uniform: uniform,
            input,
            output,
            first_bind_group,
            mid_ab_bind_group,
            mid_ba_bind_group,
            last_bind_group,
            blit_bind_group,
            first_pipeline,
            mid_pipeline,
            mid_pixels_per_workgroup: if fast_mid { 16 } else { 8 },
            last_pipeline,
            blit_pipeline,
        }
    }

    pub(crate) fn upload_input(&self, queue: &wgpu::Queue, rgba: &[u8]) {
        debug_assert_eq!(rgba.len(), (INPUT_W * INPUT_H * 4) as usize);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.input,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(INPUT_W * 4),
                rows_per_image: Some(INPUT_H),
            },
            wgpu::Extent3d {
                width: INPUT_W,
                height: INPUT_H,
                depth_or_array_layers: 1,
            },
        );
    }

    /// The upscaled image, `OUTPUT_W`x`OUTPUT_H` `Rgba8Unorm` with `COPY_SRC`:
    /// what `encode_network` writes, before the blit stretches it onto a
    /// surface.  The offline recorder reads it back instead of blitting.
    pub(crate) fn output_texture(&self) -> &wgpu::Texture {
        &self.output
    }

    /// Run the 18-layer network over the uploaded input into `output_texture`.
    pub(crate) fn encode_network(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native upscale network"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.first_pipeline);
        pass.set_bind_group(0, &self.first_bind_group, &[uniform_offset(0)]);
        pass.dispatch_workgroups(INPUT_W.div_ceil(8), INPUT_H.div_ceil(8), 1);

        pass.set_pipeline(&self.mid_pipeline);
        for layer in 0..16 {
            let bind_group = if layer % 2 == 0 {
                &self.mid_ab_bind_group
            } else {
                &self.mid_ba_bind_group
            };
            pass.set_bind_group(0, bind_group, &[uniform_offset(layer + 1)]);
            pass.dispatch_workgroups(
                INPUT_W.div_ceil(self.mid_pixels_per_workgroup),
                INPUT_H.div_ceil(8),
                1,
            );
        }

        pass.set_pipeline(&self.last_pipeline);
        pass.set_bind_group(0, &self.last_bind_group, &[uniform_offset(17)]);
        pass.dispatch_workgroups(INPUT_W.div_ceil(8), INPUT_H.div_ceil(8), 1);
    }

    pub(crate) fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport_x: f32,
        viewport_y: f32,
        viewport_width: f32,
        viewport_height: f32,
    ) {
        self.encode_network(encoder);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native upscale presentation"),
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
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &self.blit_bind_group, &[]);
        pass.set_viewport(
            viewport_x,
            viewport_y,
            viewport_width,
            viewport_height,
            0.0,
            1.0,
        );
        pass.draw(0..3, 0..1);
    }
}

/// A windowless wgpu device able to run the upscale network.
pub(crate) struct HeadlessGpu {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) adapter_info: wgpu::AdapterInfo,
    /// What the adapter offers before clamping.  Only the kernel benchmark
    /// reports it; the offline recorder just wants the device.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) adapter_limits: wgpu::Limits,
    /// The adapter offered `PASSTHROUGH_SHADERS`, so it was requested too.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) passthrough: bool,
}

/// Open a device with no surface attached, for callers that only want the
/// network's output texture: the offline recorder and the kernel benchmark.
/// The game itself takes its device from `pixels` instead, because it has to
/// present.  `SHADER_F16` is mandatory — the kernels are FP16 throughout —
/// so this returns `None` on an adapter without it.
///
/// Every limit is clamped to what the adapter reports, so the request always
/// succeeds; `max_compute_workgroup_storage_size` is what selects the fast
/// `conv_mid` variant (see `fast_mid`), and both variants are byte-identical
/// in output, so a smaller device is slower but not different.
pub(crate) fn request_headless_gpu() -> Option<HeadlessGpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let mut needed = wgpu::Features::SHADER_F16;
    if !adapter.features().contains(needed) {
        return None;
    }
    let passthrough = adapter
        .features()
        .contains(wgpu::Features::PASSTHROUGH_SHADERS);
    if passthrough {
        needed |= wgpu::Features::PASSTHROUGH_SHADERS;
    }
    let adapter_limits = adapter.limits();
    let limits = wgpu::Limits {
        max_compute_workgroup_storage_size: adapter_limits
            .max_compute_workgroup_storage_size
            .min(32_768),
        max_compute_invocations_per_workgroup: adapter_limits
            .max_compute_invocations_per_workgroup
            .min(1024),
        max_compute_workgroup_size_z: adapter_limits.max_compute_workgroup_size_z.min(64),
        ..Default::default()
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustpal headless neural upscale device"),
        required_features: needed,
        required_limits: limits,
        ..Default::default()
    }))
    .ok()?;
    Some(HeadlessGpu {
        device,
        queue,
        adapter_info: adapter.get_info(),
        adapter_limits,
        passthrough,
    })
}

fn validate_model_metadata() {
    for (index, layer) in LAYERS.iter().enumerate() {
        let matrix_end = (layer.mat_base + 9 * layer.input_vecs * layer.output_vecs) as usize * 32;
        assert!(matrix_end <= MODEL_BYTES.len(), "layer {index} matrices");
        let bias_end = (layer.bias_base + layer.output_vecs) as usize * 8;
        assert!(bias_end <= MODEL_BYTES.len(), "layer {index} bias");
        if layer.slope_base != 0 {
            let slope_end = (layer.slope_base + layer.output_vecs) as usize * 8;
            assert!(slope_end <= MODEL_BYTES.len(), "layer {index} slope");
        }
    }
}

fn uniform_offset(layer: u32) -> u32 {
    layer * UNIFORM_STRIDE as u32
}

fn layer_uniform_data() -> Vec<u8> {
    let mut bytes = vec![0; LAYERS.len() * UNIFORM_STRIDE as usize];
    for (index, layer) in LAYERS.iter().enumerate() {
        let words = [
            layer.mat_base,
            layer.bias_base,
            layer.slope_base,
            INPUT_W,
            INPUT_H,
        ];
        for (word_index, word) in words.iter().enumerate() {
            let offset = index * UNIFORM_STRIDE as usize + word_index * 4;
            bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
    bytes
}

/// Compile one of the embedded compute kernels without naga's injected
/// runtime checks. The per-access bounds clamps and per-loop iteration
/// bounding cost ~2.5x on the convolution chain (measured on Metal/M4 by
/// `bench::bench_native_upscale`) and are redundant here.
fn trusted_shader(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    let checks = wgpu::ShaderRuntimeChecks {
        bounds_checks: false,
        force_loop_bounding: false,
        ..wgpu::ShaderRuntimeChecks::checked()
    };
    // SAFETY: the WGSL is embedded in the binary; every loop has a constant
    // trip count, and all buffer/tile indices stay inside the model blob,
    // activation buffers, and workgroup tiles for the layer metadata and
    // dispatch sizes this module hardcodes (see validate_model_metadata).
    unsafe {
        device.create_shader_module_trusted(
            wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            },
            checks,
        )
    }
}

fn compute_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    entry_point: &str,
) -> wgpu::ComputePipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("native upscale compute pipeline layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry_point),
        layout: Some(&layout),
        module: shader,
        entry_point: Some(entry_point),
        compilation_options: wgpu::PipelineCompilationOptions {
            // Every kernel writes its whole workgroup tile (in-bounds pixels
            // or explicit zeros) before the barrier, so the implicit
            // zero-fill naga would otherwise emit is pure overhead.
            zero_initialize_workgroup_memory: false,
            ..Default::default()
        },
        cache: None,
    })
}

fn buffer_layout_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_layout_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(20),
        },
        count: None,
    }
}

fn input_texture_layout_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 5,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn first_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native upscale first layout"),
        entries: &[
            uniform_layout_entry(),
            buffer_layout_entry(1, true),
            buffer_layout_entry(2, true),
            buffer_layout_entry(4, false),
            input_texture_layout_entry(),
        ],
    })
}

fn mid_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native upscale middle layout"),
        entries: &[
            uniform_layout_entry(),
            buffer_layout_entry(1, true),
            buffer_layout_entry(2, true),
            buffer_layout_entry(3, true),
            buffer_layout_entry(4, false),
        ],
    })
}

fn last_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native upscale last layout"),
        entries: &[
            uniform_layout_entry(),
            buffer_layout_entry(1, true),
            buffer_layout_entry(2, true),
            buffer_layout_entry(3, true),
            input_texture_layout_entry(),
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
        ],
    })
}

pub mod offline;

#[cfg(test)]
mod bench;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_model_metadata_stays_inside_blob() {
        assert_eq!(MODEL_BYTES.len(), 1_244_000);
        validate_model_metadata();
    }

    #[test]
    fn layer_uniforms_use_dynamic_alignment_and_native_dimensions() {
        let bytes = layer_uniform_data();
        assert_eq!(bytes.len(), LAYERS.len() * UNIFORM_STRIDE as usize);
        for (index, layer) in LAYERS.iter().enumerate() {
            let base = index * UNIFORM_STRIDE as usize;
            let read = |word: usize| {
                u32::from_le_bytes(
                    bytes[base + word * 4..base + word * 4 + 4]
                        .try_into()
                        .unwrap(),
                )
            };
            assert_eq!(read(0), layer.mat_base);
            assert_eq!(read(1), layer.bias_base);
            assert_eq!(read(2), layer.slope_base);
            assert_eq!(read(3), INPUT_W);
            assert_eq!(read(4), INPUT_H);
        }
    }
}
