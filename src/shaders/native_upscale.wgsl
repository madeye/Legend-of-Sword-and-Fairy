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
@group(0) @binding(4) var<storage, read_write> dst : array<vec4<f16>>;
@group(0) @binding(5) var inputTex : texture_2d<f32>;
@group(0) @binding(6) var outTex : texture_storage_2d<rgba8unorm, write>;

const TS : u32 = 8u;
const TILE_W : u32 = 10u;
const TILE_N : u32 = 100u;
var<workgroup> tile1 : array<vec4<f16>, 100>;
var<workgroup> tile16 : array<vec4<f16>, 1600>;

fn prelu(a : vec4<f16>, s : vec4<f16>) -> vec4<f16> {
  return max(a, vec4<f16>()) + s * min(a, vec4<f16>());
}

@compute @workgroup_size(8, 8, 1)
fn conv_first(@builtin(workgroup_id) wg : vec3<u32>,
              @builtin(local_invocation_id) li : vec3<u32>,
              @builtin(local_invocation_index) lidx : u32) {
  let iw = i32(P.width);
  let ih = i32(P.height);
  let x0 = i32(wg.x * TS) - 1;
  let y0 = i32(wg.y * TS) - 1;
  for (var p = lidx; p < TILE_N; p += 64u) {
    let px = x0 + i32(p % TILE_W);
    let py = y0 + i32(p / TILE_W);
    var v = vec4<f16>();
    if (px >= 0 && px < iw && py >= 0 && py < ih) {
      let t = textureLoad(inputTex, vec2<i32>(px, py), 0);
      v = vec4<f16>(vec4<f32>(t.rgb, 0.0));
    }
    tile1[p] = v;
  }
  workgroupBarrier();

  var acc : array<vec4<f16>, 16>;
  for (var ov = 0u; ov < 16u; ov++) {
    acc[ov] = vecs[P.biasBase + ov];
  }
  var m = P.matBase;
  for (var t = 0u; t < 9u; t++) {
    let v = tile1[(li.y + t / 3u) * TILE_W + li.x + (t % 3u)];
    for (var ov = 0u; ov < 16u; ov++) {
      acc[ov] += mats[m] * v;
      m++;
    }
  }
  let x = wg.x * TS + li.x;
  let y = wg.y * TS + li.y;
  if (x < P.width && y < P.height) {
    let base = (y * P.width + x) * 16u;
    for (var ov = 0u; ov < 16u; ov++) {
      dst[base + ov] = prelu(acc[ov], vecs[P.slopeBase + ov]);
    }
  }
}

fn loadTile16(wg : vec3<u32>, lidx : u32, iw : i32, ih : i32) {
  let x0 = i32(wg.x * TS) - 1;
  let y0 = i32(wg.y * TS) - 1;
  for (var p = lidx; p < TILE_N; p += 256u) {
    let px = x0 + i32(p % TILE_W);
    let py = y0 + i32(p / TILE_W);
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
}

// Channel-split output layer: z-slice `by` accumulates output vec4s
// {by, by+4, by+8} — the R/G/B planes of row `by` in the pixel's 4x4 block —
// and stores 4 of the 16 upscaled texels. 4x the threads per tile cut the
// kernel from 6.0 ms to 1.6 ms on Metal/M4 (see bench::bench_native_upscale)
// while keeping per-vector accumulation order, so output is bit-identical.
@compute @workgroup_size(8, 8, 4)
fn conv_last(@builtin(workgroup_id) wg : vec3<u32>,
             @builtin(local_invocation_id) li : vec3<u32>,
             @builtin(local_invocation_index) lidx : u32) {
  loadTile16(wg, lidx, i32(P.width), i32(P.height));

  let by = li.z;
  var accR = vecs[P.biasBase + by];
  var accG = vecs[P.biasBase + 4u + by];
  var accB = vecs[P.biasBase + 8u + by];
  for (var t = 0u; t < 9u; t++) {
    let tp = ((li.y + t / 3u) * TILE_W + li.x + (t % 3u)) * 16u;
    for (var iv = 0u; iv < 16u; iv++) {
      let v = tile16[tp + iv];
      let mb = P.matBase + (t * 16u + iv) * 12u;
      accR += mats[mb + by] * v;
      accG += mats[mb + 4u + by] * v;
      accB += mats[mb + 8u + by] * v;
    }
  }
  let x = wg.x * TS + li.x;
  let y = wg.y * TS + li.y;
  if (x < P.width && y < P.height) {
    let res = textureLoad(inputTex, vec2<i32>(i32(x), i32(y)), 0).rgb;
    for (var bx = 0u; bx < 4u; bx++) {
      let rgb = vec3<f32>(f32(accR[bx]), f32(accG[bx]), f32(accB[bx])) + res;
      textureStore(outTex, vec2<i32>(i32(x * 4u + bx), i32(y * 4u + by)),
                   vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
    }
  }
}
