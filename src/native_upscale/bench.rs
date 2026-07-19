//! Headless GPU micro-benchmark for the native upscale kernels.
//!
//! Run with:
//!   cargo test --release --lib bench_native_upscale -- --ignored --nocapture
//!
//! Times each stage of the 18-layer network with GPU timestamp queries and
//! sweeps conv_mid variants (pixels-per-thread x channel split). Every
//! variant's full-network output is compared byte-for-byte against the
//! shipped kernel before its timing is trusted.

use super::*;

const RUNS: usize = 15;
const BATCH: usize = 4;
const OUTPUT_ROW_BYTES: u64 = OUTPUT_W as u64 * 4;
const OUTPUT_BYTES: u64 = OUTPUT_ROW_BYTES * OUTPUT_H as u64;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    output_read: wgpu::Buffer,
    passthrough: bool,
}

impl Gpu {
    fn new() -> Option<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        let mut needed = wgpu::Features::SHADER_F16;
        if !adapter.features().contains(needed) {
            eprintln!("bench: adapter lacks SHADER_F16, skipping");
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
            label: Some("upscale bench device"),
            required_features: needed,
            required_limits: limits,
            ..Default::default()
        }))
        .ok()?;
        let info = adapter.get_info();
        eprintln!(
            "bench: {} ({:?}), max invocations {}",
            info.name, info.backend, adapter_limits.max_compute_invocations_per_workgroup,
        );
        let output_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bench output read"),
            size: OUTPUT_BYTES,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Some(Gpu {
            device,
            queue,
            output_read,
            passthrough,
        })
    }

    /// Submit `BATCH` repetitions of the pass in one command buffer, wait for
    /// completion, and return the wall-clock time per repetition in ms.
    ///
    /// Metal's pass-boundary timestamps proved unreliable on a loaded desktop
    /// (impossible sub-millisecond readings), so timing is wall clock; the
    /// batch amortizes submission overhead and the caller takes a min over
    /// many runs to reject compositor/browser GPU interference.
    fn time_pass(&self, record: impl Fn(&mut wgpu::ComputePass)) -> f64 {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for _ in 0..BATCH {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            record(&mut pass);
        }
        let start = std::time::Instant::now();
        self.queue.submit([encoder.finish()]);
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        start.elapsed().as_secs_f64() * 1.0e3 / BATCH as f64
    }

    /// Run `record` RUNS times after warmup; return (median, min) in ms.
    fn measure(&self, record: impl Fn(&mut wgpu::ComputePass)) -> (f64, f64) {
        for _ in 0..2 {
            self.time_pass(&record);
        }
        let mut times: Vec<f64> = (0..RUNS).map(|_| self.time_pass(&record)).collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (times[times.len() / 2], times[0])
    }

    /// Read the network's output texture back to the CPU.
    fn read_output(&self, up: &NativeUpscaler) -> Vec<u8> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &up._output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.output_read,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(OUTPUT_ROW_BYTES as u32),
                    rows_per_image: Some(OUTPUT_H),
                },
            },
            wgpu::Extent3d {
                width: OUTPUT_W,
                height: OUTPUT_H,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = self.output_read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let bytes = slice.get_mapped_range().to_vec();
        self.output_read.unmap();
        bytes
    }
}

struct MidVariant {
    label: String,
    pipeline: wgpu::ComputePipeline,
    pixels_per_workgroup: u32,
}

/// Compile a WGSL module with explicit naga runtime-check configuration.
/// The bench only feeds it the embedded kernels, whose loops terminate and
/// whose indices stay in bounds for our fixed dispatch geometry.
fn shader_module(
    device: &wgpu::Device,
    source: &str,
    checks: wgpu::ShaderRuntimeChecks,
) -> wgpu::ShaderModule {
    let desc = wgpu::ShaderModuleDescriptor {
        label: Some("bench shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    };
    unsafe { device.create_shader_module_trusted(desc, checks) }
}

fn dispatch_first(pass: &mut wgpu::ComputePass, up: &NativeUpscaler) {
    pass.set_pipeline(&up.first_pipeline);
    pass.set_bind_group(0, &up.first_bind_group, &[uniform_offset(0)]);
    pass.dispatch_workgroups(INPUT_W.div_ceil(8), INPUT_H.div_ceil(8), 1);
}

fn dispatch_last_with(
    pass: &mut wgpu::ComputePass,
    up: &NativeUpscaler,
    pipeline: &wgpu::ComputePipeline,
) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &up.last_bind_group, &[uniform_offset(17)]);
    pass.dispatch_workgroups(INPUT_W.div_ceil(8), INPUT_H.div_ceil(8), 1);
}

fn dispatch_mids(
    pass: &mut wgpu::ComputePass,
    up: &NativeUpscaler,
    pipeline: &wgpu::ComputePipeline,
    pixels_per_workgroup: u32,
) {
    pass.set_pipeline(pipeline);
    for layer in 0..16 {
        let bind_group = if layer % 2 == 0 {
            &up.mid_ab_bind_group
        } else {
            &up.mid_ba_bind_group
        };
        pass.set_bind_group(0, bind_group, &[uniform_offset(layer + 1)]);
        pass.dispatch_workgroups(
            INPUT_W.div_ceil(pixels_per_workgroup),
            INPUT_H.div_ceil(8),
            1,
        );
    }
}

fn dispatch_last(pass: &mut wgpu::ComputePass, up: &NativeUpscaler) {
    pass.set_pipeline(&up.last_pipeline);
    pass.set_bind_group(0, &up.last_bind_group, &[uniform_offset(17)]);
    pass.dispatch_workgroups(INPUT_W.div_ceil(8), INPUT_H.div_ceil(8), 1);
}

/// conv_mid template: `px` output pixels per thread along x, 16/`split`
/// output vec4s per z-slice. px=2/split=4 reproduces the shipped fast kernel;
/// px=1/split=2 reproduces the baseline-limits fallback. With `unroll`, the
/// per-thread accumulators become individually named vec4 variables instead
/// of a loop-indexed array, so they can live in registers even when the
/// backend compiler does not unroll the output-vector loop.
fn mid_variant_source(px: u32, split: u32, unroll: bool) -> String {
    assert!(px == 1 || px == 2);
    let ovp = 16 / split;
    let invocations = 64 * split;
    let tile_w = 8 * px + 2;
    let tile_n = tile_w * 10;
    let tile_vecs = tile_n * 16;
    let coverage = 8 * px;
    let mut s = String::new();
    s.push_str(
        "enable f16;\n\
         struct Params {\n\
           matBase : u32,\n\
           biasBase : u32,\n\
           slopeBase : u32,\n\
           width : u32,\n\
           height : u32,\n\
         };\n\
         @group(0) @binding(0) var<uniform> P : Params;\n\
         @group(0) @binding(1) var<storage, read> mats : array<mat4x4<f16>>;\n\
         @group(0) @binding(2) var<storage, read> vecs : array<vec4<f16>>;\n\
         @group(0) @binding(3) var<storage, read> src : array<vec4<f16>>;\n\
         @group(0) @binding(4) var<storage, read_write> dst : array<vec4<f16>>;\n\
         fn prelu(a : vec4<f16>, s : vec4<f16>) -> vec4<f16> {\n\
           return max(a, vec4<f16>()) + s * min(a, vec4<f16>());\n\
         }\n",
    );
    s.push_str(&format!(
        "var<workgroup> tileM : array<vec4<f16>, {tile_vecs}>;\n\
         @compute @workgroup_size(8, 8, {split})\n\
         fn conv_mid(@builtin(workgroup_id) wg : vec3<u32>,\n\
                     @builtin(local_invocation_id) li : vec3<u32>,\n\
                     @builtin(local_invocation_index) lidx : u32) {{\n\
           let iw = i32(P.width);\n\
           let ih = i32(P.height);\n\
           let x0 = i32(wg.x * {coverage}u) - 1;\n\
           let y0 = i32(wg.y * 8u) - 1;\n\
           for (var p = lidx; p < {tile_n}u; p += {invocations}u) {{\n\
             let pxx = x0 + i32(p % {tile_w}u);\n\
             let pyy = y0 + i32(p / {tile_w}u);\n\
             if (pxx >= 0 && pxx < iw && pyy >= 0 && pyy < ih) {{\n\
               let base = u32(pyy * iw + pxx) * 16u;\n\
               for (var c = 0u; c < 16u; c++) {{\n\
                 tileM[p * 16u + c] = src[base + c];\n\
               }}\n\
             }} else {{\n\
               for (var c = 0u; c < 16u; c++) {{\n\
                 tileM[p * 16u + c] = vec4<f16>();\n\
               }}\n\
             }}\n\
           }}\n\
           workgroupBarrier();\n\
           let ovBase = li.z * {ovp}u;\n",
    ));
    if unroll {
        for ov in 0..ovp {
            s.push_str(&format!(
                "  let bias{ov} = vecs[P.biasBase + ovBase + {ov}u];\n"
            ));
            for p in 0..px {
                s.push_str(&format!("  var acc{p}_{ov} = bias{ov};\n"));
            }
        }
        s.push_str(&format!(
            "  for (var t = 0u; t < 9u; t++) {{\n\
                 let tp = ((li.y + t / 3u) * {tile_w}u + li.x * {px}u + (t % 3u)) * 16u;\n\
                 for (var iv = 0u; iv < 16u; iv++) {{\n",
        ));
        for p in 0..px {
            s.push_str(&format!(
                "      let v{p} = tileM[tp + {offset}u + iv];\n",
                offset = 16 * p
            ));
        }
        s.push_str("      let mb = P.matBase + (t * 16u + iv) * 16u + ovBase;\n");
        for ov in 0..ovp {
            s.push_str(&format!("      let mat{ov} = mats[mb + {ov}u];\n"));
            for p in 0..px {
                s.push_str(&format!("      acc{p}_{ov} += mat{ov} * v{p};\n"));
            }
        }
        s.push_str(&format!(
            "    }}\n\
               }}\n\
               let y = wg.y * 8u + li.y;\n\
               if (y >= P.height) {{\n\
                 return;\n\
               }}\n\
               let xOut = wg.x * {coverage}u + li.x * {px}u;\n",
        ));
        for ov in 0..ovp {
            s.push_str(&format!(
                "  let slope{ov} = vecs[P.slopeBase + ovBase + {ov}u];\n"
            ));
        }
        for p in 0..px {
            s.push_str(&format!(
                "  if (xOut + {p}u < P.width) {{\n\
                     let base{p} = (y * P.width + xOut + {p}u) * 16u + ovBase;\n",
            ));
            for ov in 0..ovp {
                s.push_str(&format!(
                    "    dst[base{p} + {ov}u] = prelu(acc{p}_{ov}, slope{ov});\n"
                ));
            }
            s.push_str("  }\n");
        }
        s.push_str("}\n");
        return s;
    }
    for p in 0..px {
        s.push_str(&format!("  var acc{p} : array<vec4<f16>, {ovp}>;\n"));
    }
    s.push_str(&format!(
        "  for (var ov = 0u; ov < {ovp}u; ov++) {{\n\
           let bias = vecs[P.biasBase + ovBase + ov];\n",
    ));
    for p in 0..px {
        s.push_str(&format!("    acc{p}[ov] = bias;\n"));
    }
    s.push_str(&format!(
        "  }}\n\
           for (var t = 0u; t < 9u; t++) {{\n\
             let tp = ((li.y + t / 3u) * {tile_w}u + li.x * {px}u + (t % 3u)) * 16u;\n\
             for (var iv = 0u; iv < 16u; iv++) {{\n",
    ));
    for p in 0..px {
        s.push_str(&format!(
            "      let v{p} = tileM[tp + {offset}u + iv];\n",
            offset = 16 * p
        ));
    }
    s.push_str(&format!(
        "      let mb = P.matBase + (t * 16u + iv) * 16u + ovBase;\n\
               for (var ov = 0u; ov < {ovp}u; ov++) {{\n\
                 let mat = mats[mb + ov];\n",
    ));
    for p in 0..px {
        s.push_str(&format!("        acc{p}[ov] += mat * v{p};\n"));
    }
    s.push_str(&format!(
        "      }}\n\
             }}\n\
           }}\n\
           let y = wg.y * 8u + li.y;\n\
           if (y >= P.height) {{\n\
             return;\n\
           }}\n\
           let xOut = wg.x * {coverage}u + li.x * {px}u;\n\
           for (var ov = 0u; ov < {ovp}u; ov++) {{\n\
             let slope = vecs[P.slopeBase + ovBase + ov];\n",
    ));
    for p in 0..px {
        s.push_str(&format!(
            "    if (xOut + {p}u < P.width) {{\n\
                   let base = (y * P.width + xOut + {p}u) * 16u;\n\
                   dst[base + ovBase + ov] = prelu(acc{p}[ov], slope);\n\
                 }}\n",
        ));
    }
    s.push_str("  }\n}\n");
    s
}

fn build_mid_variant(
    device: &wgpu::Device,
    px: u32,
    split: u32,
    unroll: bool,
    checks: wgpu::ShaderRuntimeChecks,
) -> MidVariant {
    let source = mid_variant_source(px, split, unroll);
    let layout = mid_bind_group_layout(device);
    let shader = shader_module(device, &source, checks);
    MidVariant {
        label: format!("px{px} split{split}{}", if unroll { " unroll" } else { "" }),
        pipeline: compute_pipeline(device, &layout, &shader, "conv_mid"),
        pixels_per_workgroup: 8 * px,
    }
}

/// Hand-written MSL equivalent of the px1 conv_mid kernel, used to measure
/// how much naga's WGSL->MSL translation costs versus native Metal source.
/// Buffer slots follow wgpu-hal's sequential assignment for the mid bind
/// group layout: 0 uniform, 1..4 storage in binding order.
fn mid_msl_source(split: u32) -> String {
    let ovp = 16 / split;
    let invocations = 64 * split;
    format!(
        r#"#include <metal_stdlib>
using namespace metal;

struct Params {{
    uint matBase;
    uint biasBase;
    uint slopeBase;
    uint width;
    uint height;
}};

kernel void conv_mid(
    constant Params& P [[buffer(0)]],
    const device half4x4* mats [[buffer(1)]],
    const device half4* vecs [[buffer(2)]],
    const device half4* src [[buffer(3)]],
    device half4* dst [[buffer(4)]],
    uint3 wg [[threadgroup_position_in_grid]],
    uint3 li [[thread_position_in_threadgroup]],
    uint lidx [[thread_index_in_threadgroup]])
{{
    threadgroup half4 tileM[1600];
    int iw = int(P.width);
    int ih = int(P.height);
    int x0 = int(wg.x * 8u) - 1;
    int y0 = int(wg.y * 8u) - 1;
    for (uint p = lidx; p < 100u; p += {invocations}u) {{
        int pxx = x0 + int(p % 10u);
        int pyy = y0 + int(p / 10u);
        if (pxx >= 0 && pxx < iw && pyy >= 0 && pyy < ih) {{
            uint base = uint(pyy * iw + pxx) * 16u;
            for (uint c = 0u; c < 16u; c++) {{
                tileM[p * 16u + c] = src[base + c];
            }}
        }} else {{
            for (uint c = 0u; c < 16u; c++) {{
                tileM[p * 16u + c] = half4(0.0h);
            }}
        }}
    }}
    threadgroup_barrier(mem_flags::mem_threadgroup);
    uint ovBase = li.z * {ovp}u;
    half4 acc[{ovp}];
    for (uint ov = 0u; ov < {ovp}u; ov++) {{
        acc[ov] = vecs[P.biasBase + ovBase + ov];
    }}
    for (uint t = 0u; t < 9u; t++) {{
        uint tp = ((li.y + t / 3u) * 10u + li.x + (t % 3u)) * 16u;
        for (uint iv = 0u; iv < 16u; iv++) {{
            half4 v = tileM[tp + iv];
            uint mb = P.matBase + (t * 16u + iv) * 16u + ovBase;
            for (uint ov = 0u; ov < {ovp}u; ov++) {{
                acc[ov] += mats[mb + ov] * v;
            }}
        }}
    }}
    uint x = wg.x * 8u + li.x;
    uint y = wg.y * 8u + li.y;
    if (x < P.width && y < P.height) {{
        uint base = (y * P.width + x) * 16u;
        for (uint ov = 0u; ov < {ovp}u; ov++) {{
            half4 a = acc[ov];
            half4 s = vecs[P.slopeBase + ovBase + ov];
            dst[base + ovBase + ov] = max(a, half4(0.0h)) + s * min(a, half4(0.0h));
        }}
    }}
}}
"#
    )
}

fn build_msl_mid_variant(device: &wgpu::Device, split: u32) -> MidVariant {
    let source = mid_msl_source(split);
    let layout = mid_bind_group_layout(device);
    let desc = wgpu::ShaderModuleDescriptorPassthrough {
        label: Some("bench msl mid"),
        num_workgroups: (8, 8, split),
        msl: Some(source.into()),
        ..Default::default()
    };
    let shader = unsafe { device.create_shader_module_passthrough(desc) };
    MidVariant {
        label: format!("msl px1 split{split}"),
        pipeline: compute_pipeline(device, &layout, &shader, "conv_mid"),
        pixels_per_workgroup: 8,
    }
}

/// Channel-split conv_last candidate: workgroup 8x8x4 where z-slice `by`
/// computes output vec4s {by, by+4, by+8} (the R/G/B planes of output row
/// `by` in the pixel's 4x4 block) and stores 4 of the 16 output texels.
/// Accumulation order per output vector matches the shipped kernel exactly.
const LAST_SPLIT_WGSL: &str = r#"
enable f16;

struct Params {
  matBase : u32,
  biasBase : u32,
  slopeBase : u32,
  width : u32,
  height : u32,
};

@group(0) @binding(0) var<uniform> P : Params;
@group(0) @binding(1) var<storage, read> mats : array<mat4x4<f16>>;
@group(0) @binding(2) var<storage, read> vecs : array<vec4<f16>>;
@group(0) @binding(3) var<storage, read> src : array<vec4<f16>>;
@group(0) @binding(5) var inputTex : texture_2d<f32>;
@group(0) @binding(6) var outTex : texture_storage_2d<rgba8unorm, write>;

var<workgroup> tile16 : array<vec4<f16>, 1600>;

@compute @workgroup_size(8, 8, 4)
fn conv_last(@builtin(workgroup_id) wg : vec3<u32>,
             @builtin(local_invocation_id) li : vec3<u32>,
             @builtin(local_invocation_index) lidx : u32) {
  let iw = i32(P.width);
  let ih = i32(P.height);
  let x0 = i32(wg.x * 8u) - 1;
  let y0 = i32(wg.y * 8u) - 1;
  for (var p = lidx; p < 100u; p += 256u) {
    let px = x0 + i32(p % 10u);
    let py = y0 + i32(p / 10u);
    if (px >= 0 && px < iw && py >= 0 && py < ih) {
      let base = u32(py * iw + px) * 16u;
      for (var c = 0u; c < 16u; c++) {
        tile16[p * 16u + c] = src[base + c];
      }
    } else {
      for (var c = 0u; c < 16u; c++) {
        tile16[p * 16u + c] = vec4<f16>();
      }
    }
  }
  workgroupBarrier();

  let by = li.z;
  var accR = vecs[P.biasBase + by];
  var accG = vecs[P.biasBase + 4u + by];
  var accB = vecs[P.biasBase + 8u + by];
  for (var t = 0u; t < 9u; t++) {
    let tp = ((li.y + t / 3u) * 10u + li.x + (t % 3u)) * 16u;
    for (var iv = 0u; iv < 16u; iv++) {
      let v = tile16[tp + iv];
      let mb = P.matBase + (t * 16u + iv) * 12u;
      accR += mats[mb + by] * v;
      accG += mats[mb + 4u + by] * v;
      accB += mats[mb + 8u + by] * v;
    }
  }
  let x = wg.x * 8u + li.x;
  let y = wg.y * 8u + li.y;
  if (x < P.width && y < P.height) {
    let res = textureLoad(inputTex, vec2<i32>(i32(x), i32(y)), 0).rgb;
    for (var bx = 0u; bx < 4u; bx++) {
      let rgb = vec3<f32>(f32(accR[bx]), f32(accG[bx]), f32(accB[bx])) + res;
      textureStore(outTex, vec2<i32>(i32(x * 4u + bx), i32(y * 4u + by)),
                   vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
    }
  }
}
"#;

fn synthetic_input() -> Vec<u8> {
    let mut state = 0x1234_5678_u32;
    (0..(INPUT_W * INPUT_H * 4) as usize)
        .map(|i| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if i % 4 == 3 {
                255
            } else {
                (state >> 24) as u8
            }
        })
        .collect()
}

#[test]
#[ignore = "GPU micro-benchmark; run with --ignored --nocapture"]
fn bench_native_upscale() {
    let Some(gpu) = Gpu::new() else {
        return;
    };
    let up = NativeUpscaler::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    up.upload_input(&gpu.queue, &synthetic_input());

    let shipped_ppw = up.mid_pixels_per_workgroup;
    let (full_med, full_min) = gpu.measure(|pass| {
        dispatch_first(pass, &up);
        dispatch_mids(pass, &up, &up.mid_pipeline, shipped_ppw);
        dispatch_last(pass, &up);
    });
    let reference = gpu.read_output(&up);
    let (first_med, _) = gpu.measure(|pass| dispatch_first(pass, &up));
    let (last_med, _) = gpu.measure(|pass| dispatch_last(pass, &up));
    let (mid_med, mid_min) =
        gpu.measure(|pass| dispatch_mids(pass, &up, &up.mid_pipeline, shipped_ppw));
    eprintln!("shipped kernels: full network median {full_med:.3} ms (min {full_min:.3})");
    eprintln!("  conv_first x1 {first_med:.3} ms | conv_mid x16 {mid_med:.3} ms (min {mid_min:.3}) | conv_last x1 {last_med:.3} ms");

    // Which naga runtime check costs what, on the shipped kernel source.
    let shipped_source = include_str!("../shaders/native_upscale_mid_fast.wgsl");
    let check_configs: [(&str, wgpu::ShaderRuntimeChecks); 3] = [
        (
            "no-bounds-checks",
            wgpu::ShaderRuntimeChecks {
                bounds_checks: false,
                ..wgpu::ShaderRuntimeChecks::checked()
            },
        ),
        (
            "no-loop-bounding",
            wgpu::ShaderRuntimeChecks {
                force_loop_bounding: false,
                ..wgpu::ShaderRuntimeChecks::checked()
            },
        ),
        ("unchecked", wgpu::ShaderRuntimeChecks::unchecked()),
    ];
    for (label, checks) in check_configs {
        let layout = mid_bind_group_layout(&gpu.device);
        let shader = shader_module(&gpu.device, shipped_source, checks);
        let pipeline = compute_pipeline(&gpu.device, &layout, &shader, "conv_mid");
        let (median, min) = gpu.measure(|pass| dispatch_mids(pass, &up, &pipeline, shipped_ppw));
        eprintln!("shipped mid {label:18} median {median:7.3} ms  min {min:7.3} ms");
    }

    let max_invocations = gpu.device.limits().max_compute_invocations_per_workgroup;
    let mut sweep = Vec::new();
    for px in [1u32, 2u32] {
        for split in [2u32, 4, 8] {
            for unroll in [false, true] {
                if 64 * split <= max_invocations {
                    sweep.push((px, split, unroll));
                }
            }
        }
    }
    let mut variants: Vec<MidVariant> = sweep
        .iter()
        .map(|&(px, split, unroll)| {
            build_mid_variant(
                &gpu.device,
                px,
                split,
                unroll,
                wgpu::ShaderRuntimeChecks::unchecked(),
            )
        })
        .collect();
    if gpu.passthrough {
        for split in [2u32, 4] {
            variants.push(build_msl_mid_variant(&gpu.device, split));
        }
    }
    let mut results = Vec::new();
    {
        for variant in &variants {
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                dispatch_first(&mut pass, &up);
                dispatch_mids(
                    &mut pass,
                    &up,
                    &variant.pipeline,
                    variant.pixels_per_workgroup,
                );
                dispatch_last(&mut pass, &up);
            }
            gpu.queue.submit([encoder.finish()]);
            let output = gpu.read_output(&up);
            let mismatches = output
                .iter()
                .zip(&reference)
                .filter(|(a, b)| a != b)
                .count();
            let max_diff = output
                .iter()
                .zip(&reference)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            let (median, min) = gpu.measure(|pass| {
                dispatch_mids(pass, &up, &variant.pipeline, variant.pixels_per_workgroup)
            });
            eprintln!(
                "mid variant {:26} median {median:7.3} ms  min {min:7.3} ms  mismatches {mismatches} (max diff {max_diff})",
                variant.label,
            );
            results.push((variant.label.clone(), median, mismatches));
        }
    }
    let best = results
        .iter()
        .filter(|(_, _, mismatches)| *mismatches == 0)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    if let Some((label, median, _)) = best {
        eprintln!("best exact mid variant: {label} at {median:.3} ms (shipped {mid_med:.3} ms)");
    }

    // conv_last: channel-split candidate against the shipped kernel.
    let last_layout = last_bind_group_layout(&gpu.device);
    let split_shader = shader_module(
        &gpu.device,
        LAST_SPLIT_WGSL,
        wgpu::ShaderRuntimeChecks::unchecked(),
    );
    let split_last = compute_pipeline(&gpu.device, &last_layout, &split_shader, "conv_last");
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        dispatch_first(&mut pass, &up);
        dispatch_mids(&mut pass, &up, &up.mid_pipeline, shipped_ppw);
        dispatch_last_with(&mut pass, &up, &split_last);
    }
    gpu.queue.submit([encoder.finish()]);
    let output = gpu.read_output(&up);
    let mismatches = output
        .iter()
        .zip(&reference)
        .filter(|(a, b)| a != b)
        .count();
    let (median, min) = gpu.measure(|pass| dispatch_last_with(pass, &up, &split_last));
    eprintln!(
        "conv_last split-z variant: median {median:.3} ms  min {min:.3} ms  mismatches {mismatches} (shipped {last_med:.3} ms)"
    );
}
