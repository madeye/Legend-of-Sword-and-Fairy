// WebGPU neural upscaler: runs realesr-animevideov3 (SRVGGNetCompact,
// 18 convs) as one hand-written WGSL module and one command submit per frame.
//
// Convolutions use QDQ INT8 weights and packed dot4I8Packed instructions.
// Each convolution is dequantized into fp32 for bias and PReLU, then the Q node
// immediately packs that fp32 activation result for the next convolution.
// Residual addition and output conversion also remain fp32.
//
// Frames are coalesced: if one is still on the GPU when the next arrives, only
// the newest waiting frame is retained, keeping latency bounded.

"use strict";

const NN_W = 320;
const NN_H = 200;
const NN_SCALE = 4;

const NN_PRELUDE = /* wgsl */ `
requires packed_4x8_integer_dot_product;

struct Params {
  quantBase : u32,
  weightScaleBase : u32,
  biasBase : u32,
  slopeBase : u32,
  activationScaleBase : u32,
  outputScaleBase : u32,
  width : u32,
  height : u32,
};

@group(0) @binding(0) var<uniform> P : Params;
@group(0) @binding(1) var<storage, read> qweights : array<u32>;
@group(0) @binding(2) var<storage, read> vecs : array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> src : array<u32>;
@group(0) @binding(4) var<storage, read_write> dst : array<u32>;
@group(0) @binding(5) var inputTex : texture_2d<f32>;
@group(0) @binding(6) var outTex : texture_storage_2d<rgba8unorm, write>;

fn prelu(a : vec4<f32>, s : vec4<f32>) -> vec4<f32> {
  return max(a, vec4<f32>()) + s * min(a, vec4<f32>());
}

fn dot4x4(packed : u32, weightBase : u32) -> vec4<i32> {
  return vec4<i32>(
    dot4I8Packed(packed, qweights[weightBase]),
    dot4I8Packed(packed, qweights[weightBase + 1u]),
    dot4I8Packed(packed, qweights[weightBase + 2u]),
    dot4I8Packed(packed, qweights[weightBase + 3u]));
}

fn quantize4(v : vec4<f32>, scale : f32) -> u32 {
  return pack4xI8Clamp(vec4<i32>(round(v / scale)));
}
`;

// First and last convolutions.  The first directly quantizes normalized RGB;
// the last consumes a workgroup-local QDQ view of the fp32 activation buffer
// and fuses depth-to-space, residual addition, clamp, and rgba8 output.
const NN_EDGE_WGSL = NN_PRELUDE + /* wgsl */ `
const TS : u32 = 8u;
var<workgroup> tileRGB : array<u32, 100>;
var<workgroup> tileQ : array<u32, 1600>;

@compute @workgroup_size(8, 8, 1)
fn conv_first(@builtin(workgroup_id) wg : vec3<u32>,
              @builtin(local_invocation_id) li : vec3<u32>,
              @builtin(local_invocation_index) lidx : u32) {
  let iw = i32(P.width); let ih = i32(P.height);
  let x0 = i32(wg.x * TS) - 1; let y0 = i32(wg.y * TS) - 1;
  for (var p = lidx; p < 100u; p += 64u) {
    let px = x0 + i32(p % 10u);
    let py = y0 + i32(p / 10u);
    var v = vec4<f32>();
    if (px >= 0 && px < iw && py >= 0 && py < ih) {
      v = vec4<f32>(textureLoad(inputTex, vec2<i32>(px, py), 0).rgb, 0.0);
    }
    tileRGB[p] = pack4xI8Clamp(vec4<i32>(round(v * 127.0)));
  }
  workgroupBarrier();

  var sum : array<vec4<i32>, 16>;
  for (var ov = 0u; ov < 16u; ov++) { sum[ov] = vec4<i32>(); }
  for (var t = 0u; t < 9u; t++) {
    let packed = tileRGB[(li.y + t / 3u) * 10u + li.x + (t % 3u)];
    for (var ov = 0u; ov < 16u; ov++) {
      let wb = P.quantBase + (t * 16u + ov) * 4u;
      sum[ov] += dot4x4(packed, wb);
    }
  }
  let x = wg.x * TS + li.x; let y = wg.y * TS + li.y;
  if (x < P.width && y < P.height) {
    let base = (y * P.width + x) * 16u;
    for (var ov = 0u; ov < 16u; ov++) {
      let conv = vec4<f32>(sum[ov]) * vecs[P.activationScaleBase].x *
                 vecs[P.weightScaleBase + ov];
      let activated = prelu(vecs[P.biasBase + ov] + conv,
                            vecs[P.slopeBase + ov]);
      dst[base + ov] = quantize4(activated,
                                 vecs[P.outputScaleBase][ov / 4u]);
    }
  }
}

fn loadTile16(wg : vec3<u32>, lidx : u32, iw : i32, ih : i32) {
  let x0 = i32(wg.x * TS) - 1; let y0 = i32(wg.y * TS) - 1;
  for (var q = lidx; q < 1600u; q += 64u) {
    let p = q / 16u; let iv = q % 16u;
    let px = x0 + i32(p % 10u); let py = y0 + i32(p / 10u);
    if (px >= 0 && px < iw && py >= 0 && py < ih) {
      tileQ[q] = src[u32(py * iw + px) * 16u + iv];
    } else {
      tileQ[q] = 0u;
    }
  }
  workgroupBarrier();
}

@compute @workgroup_size(8, 8, 1)
fn conv_last(@builtin(workgroup_id) wg : vec3<u32>,
             @builtin(local_invocation_id) li : vec3<u32>,
             @builtin(local_invocation_index) lidx : u32) {
  loadTile16(wg, lidx, i32(P.width), i32(P.height));
  var conv : array<vec4<f32>, 12>;
  for (var ov = 0u; ov < 12u; ov++) {
    conv[ov] = vec4<f32>();
    for (var g = 0u; g < 4u; g++) {
      var sum = vec4<i32>();
      for (var t = 0u; t < 9u; t++) {
        let pixel = (li.y + t / 3u) * 10u + li.x + (t % 3u);
        for (var k = 0u; k < 4u; k++) {
          let iv = g * 4u + k;
          let wb = P.quantBase + ((t * 16u + iv) * 12u + ov) * 4u;
          sum += dot4x4(tileQ[pixel * 16u + iv], wb);
        }
      }
      conv[ov] += vec4<f32>(sum) * vecs[P.activationScaleBase][g];
    }
    conv[ov] *= vecs[P.weightScaleBase + ov];
  }
  let x = wg.x * TS + li.x; let y = wg.y * TS + li.y;
  if (x < P.width && y < P.height) {
    let res = textureLoad(inputTex, vec2<i32>(i32(x), i32(y)), 0).rgb;
    for (var by = 0u; by < 4u; by++) {
      for (var bx = 0u; bx < 4u; bx++) {
        let rgb = vec3<f32>(
          vecs[P.biasBase + by][bx] + conv[by][bx],
          vecs[P.biasBase + 4u + by][bx] + conv[4u + by][bx],
          vecs[P.biasBase + 8u + by][bx] + conv[8u + by][bx]) + res;
        textureStore(outTex, vec2<i32>(i32(x * 4u + bx), i32(y * 4u + by)),
                     vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
      }
    }
  }
}
`;

const NN_MID_PRELUDE = /* wgsl */ `
requires packed_4x8_integer_dot_product;

struct Params {
  quantBase : u32,
  weightScaleBase : u32,
  biasBase : u32,
  slopeBase : u32,
  activationScaleBase : u32,
  outputScaleBase : u32,
  width : u32,
  height : u32,
};

@group(0) @binding(0) var<uniform> P : Params;
@group(0) @binding(1) var<storage, read> qweights : array<u32>;
@group(0) @binding(2) var<storage, read> vecs : array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> srcQ : array<u32>;
@group(0) @binding(4) var<storage, read_write> dstQ : array<u32>;

fn dot4x4(packed : u32, weightBase : u32) -> vec4<i32> {
  return vec4<i32>(
    dot4I8Packed(packed, qweights[weightBase]),
    dot4I8Packed(packed, qweights[weightBase + 1u]),
    dot4I8Packed(packed, qweights[weightBase + 2u]),
    dot4I8Packed(packed, qweights[weightBase + 3u]));
}

fn prelu(a : vec4<f32>, s : vec4<f32>) -> vec4<f32> {
  return max(a, vec4<f32>()) + s * min(a, vec4<f32>());
}
`;

// Four z-threads split the output into 16-channel QDQ groups.  PReLU executes
// in fp32, then its result is immediately requantized for the next convolution.
function nnMidWgsl({ tw, th, split, px }) {
  const tileW = tw * px + 2, tileH = th + 2;
  const tileN = tileW * tileH;
  const threads = tw * th * split;
  const ovn = 16 / split;
  return NN_MID_PRELUDE + `
var<workgroup> tileM : array<u32, ${tileN * 16}>;

@compute @workgroup_size(${tw}, ${th}, ${split})
fn conv_mid(@builtin(workgroup_id) wg : vec3<u32>,
            @builtin(local_invocation_id) li : vec3<u32>,
            @builtin(local_invocation_index) lidx : u32) {
  let iw = i32(P.width); let ih = i32(P.height);
  let x0 = i32(wg.x * ${tw * px}u) - 1;
  let y0 = i32(wg.y * ${th}u) - 1;
  for (var q = lidx; q < ${tileN * 16}u; q += ${threads}u) {
    let p = q / 16u; let iv = q % 16u;
    let pxx = x0 + i32(p % ${tileW}u); let pyy = y0 + i32(p / ${tileW}u);
    if (pxx >= 0 && pxx < iw && pyy >= 0 && pyy < ih) {
      tileM[q] = srcQ[u32(pyy * iw + pxx) * 16u + iv];
    } else {
      tileM[q] = 0u;
    }
  }
  workgroupBarrier();

  let ovBase = li.z * ${ovn}u;
  let y = wg.y * ${th}u + li.y;
  if (y >= P.height) { return; }
  for (var ov = 0u; ov < ${ovn}u; ov++) {
    let outv = ovBase + ov;
${Array.from({ length: px }, (_, j) => `    var conv${j} = vec4<f32>();`).join("\n")}
    for (var g = 0u; g < 4u; g++) {
${Array.from({ length: px }, (_, j) => `      var sum${j} = vec4<i32>();`).join("\n")}
      for (var t = 0u; t < 9u; t++) {
        let pixel = (li.y + t / 3u) * ${tileW}u + li.x * ${px}u + (t % 3u);
        for (var k = 0u; k < 4u; k++) {
          let iv = g * 4u + k;
          let wb = P.quantBase + ((t * 16u + iv) * 16u + outv) * 4u;
          let w0 = qweights[wb]; let w1 = qweights[wb + 1u];
          let w2 = qweights[wb + 2u]; let w3 = qweights[wb + 3u];
${Array.from({ length: px }, (_, j) => `          {
            let packed = tileM[(pixel + ${j}u) * 16u + iv];
            sum${j} += vec4<i32>(dot4I8Packed(packed, w0),
                                 dot4I8Packed(packed, w1),
                                 dot4I8Packed(packed, w2),
                                 dot4I8Packed(packed, w3));
          }`).join("\n")}
        }
      }
      let actScale = vecs[P.activationScaleBase][g];
${Array.from({ length: px }, (_, j) => `      conv${j} += vec4<f32>(sum${j}) * actScale;`).join("\n")}
    }
    let ws = vecs[P.weightScaleBase + outv];
    let os = vecs[P.outputScaleBase][outv / 4u];
${Array.from({ length: px }, (_, j) => `    {
      let x = wg.x * ${tw * px}u + li.x * ${px}u + ${j}u;
      if (x < P.width) {
        let a = prelu(vecs[P.biasBase + outv] + conv${j} * ws,
                      vecs[P.slopeBase + outv]);
        dstQ[(y * P.width + x) * 16u + outv] =
          pack4xI8Clamp(vec4<i32>(round(a / os)));
      }
    }`).join("\n")}
  }
}
`;
}

const nnQDQParams = new URLSearchParams(location.search);
const NN_MID_CFG = {
  tw: Number(nnQDQParams.get("qtw")) || 4,
  th: Number(nnQDQParams.get("qth")) || 8,
  split: Number(nnQDQParams.get("qsplit")) || 8,
  px: Number(nnQDQParams.get("qpx")) || 8,
};

const NN_BLIT_WGSL = /* wgsl */ `
@group(0) @binding(0) var t : texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) i : u32) -> @builtin(position) vec4<f32> {
  var p = array<vec2<f32>, 3>(vec2f(-1.0, 3.0), vec2f(-1.0, -1.0), vec2f(3.0, -1.0));
  return vec4<f32>(p[i], 0.0, 1.0);
}
@fragment fn fs(@builtin(position) pos : vec4<f32>) -> @location(0) vec4<f32> {
  return textureLoad(t, vec2<i32>(pos.xy), 0);
}
`;

class NNUpscaler {
  static async create(canvas) {
    if (!navigator.gpu) throw new Error("WebGPU unavailable");
    if (!navigator.gpu.wgslLanguageFeatures?.has("packed_4x8_integer_dot_product")) {
      throw new Error("no WGSL packed INT8 dot product support");
    }
    const adapter = await navigator.gpu.requestAdapter();
    if (!adapter) throw new Error("no WebGPU adapter");
    const requiredFeatures = [];
    if (adapter.features.has("timestamp-query")) requiredFeatures.push("timestamp-query");
    const device = await adapter.requestDevice({
      requiredFeatures,
      requiredLimits: { maxComputeWorkgroupStorageSize:
        Math.min(adapter.limits.maxComputeWorkgroupStorageSize, 32768) },
    });
    const [manifest, weights] = await Promise.all([
      fetch("models/realesr-animevideov3-fp16.mega.qdq.json").then((r) => {
        if (!r.ok) throw new Error(`weights manifest: HTTP ${r.status}`);
        return r.json();
      }),
      fetch("models/realesr-animevideov3-fp16.mega.qdq.bin").then((r) => {
        if (!r.ok) throw new Error(`weights: HTTP ${r.status}`);
        return r.arrayBuffer();
      }),
    ]);
    const nn = new NNUpscaler(canvas, device, manifest, weights);
    await nn.initPipelines();
    return nn;
  }

  constructor(canvas, device, manifest, weights) {
    this.canvas = canvas;
    this.device = device;
    this.dead = false;
    device.lost.then((info) => {
      this.dead = true;
      this.onDeviceLost?.(info);
    });

    canvas.width = NN_W * NN_SCALE;
    canvas.height = NN_H * NN_SCALE;
    this.ctx = canvas.getContext("webgpu");
    this.format = navigator.gpu.getPreferredCanvasFormat();
    this.ctx.configure({ device, format: this.format, alphaMode: "opaque" });

    const C = GPUShaderStage.COMPUTE;
    const uni = { buffer: { type: "uniform", hasDynamicOffset: true } };
    const ro = { buffer: { type: "read-only-storage" } };
    const rw = { buffer: { type: "storage" } };
    this.bglFirst = device.createBindGroupLayout({ entries: [
      { binding: 0, visibility: C, ...uni },
      { binding: 1, visibility: C, ...ro },
      { binding: 2, visibility: C, ...ro },
      { binding: 4, visibility: C, ...rw },
      { binding: 5, visibility: C, texture: {} },
    ]});
    this.bglMid = device.createBindGroupLayout({ entries: [
      { binding: 0, visibility: C, ...uni },
      { binding: 1, visibility: C, ...ro },
      { binding: 2, visibility: C, ...ro },
      { binding: 3, visibility: C, ...ro },
      { binding: 4, visibility: C, ...rw },
    ]});
    this.bglLast = device.createBindGroupLayout({ entries: [
      { binding: 0, visibility: C, ...uni },
      { binding: 1, visibility: C, ...ro },
      { binding: 2, visibility: C, ...ro },
      { binding: 3, visibility: C, ...ro },
      { binding: 5, visibility: C, texture: {} },
      { binding: 6, visibility: C,
        storageTexture: { access: "write-only", format: "rgba8unorm" } },
    ]});

    const nLayers = manifest.layers.length;
    this.uOffsets = manifest.layers.map((_, i) => i * 256);
    const ubuf = device.createBuffer({
      size: nLayers * 256,
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
    const uniformData = new ArrayBuffer(nLayers * 256);
    const udata = new Uint32Array(uniformData);
    manifest.layers.forEach((l, i) => {
      const base = i * 64;
      udata.set([l.quantBase, l.weightScaleBase, l.biasBase, l.slopeBase,
                 l.activationScaleBase, l.outputScaleBase, NN_W, NN_H], base);
    });
    device.queue.writeBuffer(ubuf, 0, uniformData);

    const wbuf = device.createBuffer({
      size: weights.byteLength,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
    device.queue.writeBuffer(wbuf, 0, weights);

    const actSize = NN_W * NN_H * 16 * 4; // 16 packed vec4<i8> per pixel
    this.actA = device.createBuffer({ size: actSize, usage: GPUBufferUsage.STORAGE });
    this.actB = device.createBuffer({ size: actSize, usage: GPUBufferUsage.STORAGE });

    this.inputTex = device.createTexture({
      size: [NN_W, NN_H], format: "rgba8unorm",
      usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
    this.outTex = device.createTexture({
      size: [NN_W * NN_SCALE, NN_H * NN_SCALE], format: "rgba8unorm",
      usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING |
             GPUTextureUsage.COPY_SRC });

    const u = { binding: 0, resource: { buffer: ubuf, size: 256 } };
    const qw = { binding: 1, resource: { buffer: wbuf } };
    const vecs = { binding: 2, resource: { buffer: wbuf } };
    const inView = this.inputTex.createView();
    this.bgFirst = device.createBindGroup({ layout: this.bglFirst, entries: [
      u, qw, vecs, { binding: 4, resource: { buffer: this.actA } },
      { binding: 5, resource: inView },
    ]});
    this.bgMidAB = device.createBindGroup({ layout: this.bglMid, entries: [
      u, qw, vecs, { binding: 3, resource: { buffer: this.actA } },
      { binding: 4, resource: { buffer: this.actB } },
    ]});
    this.bgMidBA = device.createBindGroup({ layout: this.bglMid, entries: [
      u, qw, vecs, { binding: 3, resource: { buffer: this.actB } },
      { binding: 4, resource: { buffer: this.actA } },
    ]});
    this.bgLast = device.createBindGroup({ layout: this.bglLast, entries: [
      u, qw, vecs, { binding: 3, resource: { buffer: this.actA } },
      { binding: 5, resource: inView },
      { binding: 6, resource: this.outTex.createView() },
    ]});

    this.pending = null;
    this.busy = false;

    this.querySet = null;
    if (device.features.has("timestamp-query")) {
      this.querySet = device.createQuerySet({ type: "timestamp", count: 2 });
      this.queryResolve = device.createBuffer({
        size: 16, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC });
      this.queryRead = device.createBuffer({
        size: 16, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    }
  }

  async initPipelines() {
    const device = this.device;
    this.midCfg = NN_MID_CFG;
    const edgeModule = device.createShaderModule({ code: NN_EDGE_WGSL });
    const midModule = device.createShaderModule({ code: nnMidWgsl(this.midCfg) });
    const infos = await Promise.all([
      edgeModule.getCompilationInfo(), midModule.getCompilationInfo()]);
    const shaderErrors = infos.flatMap((info) => info.messages)
      .filter((message) => message.type === "error");
    if (shaderErrors.length) {
      throw new Error(shaderErrors.map((message) =>
        `${message.lineNum}:${message.linePos} ${message.message}`).join("\n"));
    }
    const mk = (bgl, module, entryPoint) => device.createComputePipelineAsync({
      layout: device.createPipelineLayout({ bindGroupLayouts: [bgl] }),
      compute: { module, entryPoint },
    });
    const blitModule = device.createShaderModule({ code: NN_BLIT_WGSL });
    [this.pFirst, this.pMid, this.pLast, this.pBlit] = await Promise.all([
      mk(this.bglFirst, edgeModule, "conv_first"),
      mk(this.bglMid, midModule, "conv_mid"),
      mk(this.bglLast, edgeModule, "conv_last"),
      device.createRenderPipelineAsync({
        layout: "auto",
        vertex: { module: blitModule, entryPoint: "vs" },
        fragment: { module: blitModule, entryPoint: "fs",
                    targets: [{ format: this.format }] },
      }),
    ]);
    this.bgBlit = device.createBindGroup({
      layout: this.pBlit.getBindGroupLayout(0),
      entries: [{ binding: 0, resource: this.outTex.createView() }],
    });
  }

  present(pixels) {
    this.pending = pixels;
    if (!this.busy) this.pump();
  }

  encodeNetwork(encoder, timed = false) {
    const gx = Math.ceil(NN_W / 8), gy = Math.ceil(NN_H / 8);
    const mgx = Math.ceil(NN_W / (this.midCfg.tw * this.midCfg.px));
    const mgy = Math.ceil(NN_H / this.midCfg.th);
    const pass = encoder.beginComputePass(timed && this.querySet ? {
      timestampWrites: { querySet: this.querySet,
        beginningOfPassWriteIndex: 0, endOfPassWriteIndex: 1 },
    } : {});
    pass.setPipeline(this.pFirst);
    pass.setBindGroup(0, this.bgFirst, [this.uOffsets[0]]);
    pass.dispatchWorkgroups(gx, gy);
    pass.setPipeline(this.pMid);
    for (let i = 0; i < 16; i++) {
      pass.setBindGroup(0, i % 2 === 0 ? this.bgMidAB : this.bgMidBA,
        [this.uOffsets[1 + i]]);
      pass.dispatchWorkgroups(mgx, mgy);
    }
    pass.setPipeline(this.pLast);
    pass.setBindGroup(0, this.bgLast, [this.uOffsets[17]]);
    pass.dispatchWorkgroups(gx, gy);
    pass.end();
  }

  // Benchmark/test hook: runs the network without the presentation blit and
  // returns a GPU timestamp in milliseconds when timestamp-query is available.
  async runNetwork(pixels) {
    const device = this.device;
    if (pixels) {
      device.queue.writeTexture({ texture: this.inputTex }, pixels,
        { bytesPerRow: NN_W * 4 }, [NN_W, NN_H]);
    }
    const encoder = device.createCommandEncoder();
    this.encodeNetwork(encoder, true);
    if (this.querySet) {
      encoder.resolveQuerySet(this.querySet, 0, 2, this.queryResolve, 0);
      encoder.copyBufferToBuffer(this.queryResolve, 0, this.queryRead, 0, 16);
    }
    device.queue.submit([encoder.finish()]);
    await device.queue.onSubmittedWorkDone();
    if (!this.querySet) return null;
    await this.queryRead.mapAsync(GPUMapMode.READ);
    const t = new BigInt64Array(this.queryRead.getMappedRange());
    const ms = Number(t[1] - t[0]) / 1e6;
    this.queryRead.unmap();
    return ms;
  }

  async readback() {
    const bytesPerRow = NN_W * NN_SCALE * 4;
    const buf = this.device.createBuffer({
      size: bytesPerRow * NN_H * NN_SCALE,
      usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    const encoder = this.device.createCommandEncoder();
    encoder.copyTextureToBuffer({ texture: this.outTex },
      { buffer: buf, bytesPerRow }, [NN_W * NN_SCALE, NN_H * NN_SCALE]);
    this.device.queue.submit([encoder.finish()]);
    await buf.mapAsync(GPUMapMode.READ);
    const data = new Uint8Array(buf.getMappedRange()).slice();
    buf.destroy();
    return data;
  }

  async pump() {
    this.busy = true;
    while (this.pending && !this.dead) {
      const pixels = this.pending;
      this.pending = null;
      const device = this.device;
      device.queue.writeTexture({ texture: this.inputTex }, pixels,
        { bytesPerRow: NN_W * 4 }, [NN_W, NN_H]);

      const encoder = device.createCommandEncoder();
      this.encodeNetwork(encoder);

      const rp = encoder.beginRenderPass({ colorAttachments: [{
        view: this.ctx.getCurrentTexture().createView(),
        loadOp: "clear", clearValue: [0, 0, 0, 1], storeOp: "store" }] });
      rp.setPipeline(this.pBlit);
      rp.setBindGroup(0, this.bgBlit);
      rp.draw(3);
      rp.end();
      device.queue.submit([encoder.finish()]);
      await device.queue.onSubmittedWorkDone();
    }
    this.busy = false;
  }
}
