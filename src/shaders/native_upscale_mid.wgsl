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

// Exact 8x8x2, one-pixel fallback from web/nn-upscale.js. It uses 12.8 KiB
// of workgroup storage and fits Vulkan's baseline 16 KiB limit.
var<workgroup> tileM : array<vec4<f16>, 1600>;

fn prelu(a : vec4<f16>, s : vec4<f16>) -> vec4<f16> {
  return max(a, vec4<f16>()) + s * min(a, vec4<f16>());
}

@compute @workgroup_size(8, 8, 2)
fn conv_mid(@builtin(workgroup_id) wg : vec3<u32>,
            @builtin(local_invocation_id) li : vec3<u32>,
            @builtin(local_invocation_index) lidx : u32) {
  let iw = i32(P.width);
  let ih = i32(P.height);
  let x0 = i32(wg.x * 8u) - 1;
  let y0 = i32(wg.y * 8u) - 1;
  for (var p = lidx; p < 100u; p += 128u) {
    let pxx = x0 + i32(p % 10u);
    let pyy = y0 + i32(p / 10u);
    if (pxx >= 0 && pxx < iw && pyy >= 0 && pyy < ih) {
      let base = u32(pyy * iw + pxx) * 16u;
      for (var c = 0u; c < 16u; c++) {
        tileM[p * 16u + c] = src[base + c];
      }
    } else {
      for (var c = 0u; c < 16u; c++) {
        tileM[p * 16u + c] = vec4<f16>();
      }
    }
  }
  workgroupBarrier();

  let ovBase = li.z * 8u;
  var acc : array<vec4<f16>, 8>;
  for (var ov = 0u; ov < 8u; ov++) {
    acc[ov] = vecs[P.biasBase + ovBase + ov];
  }
  for (var t = 0u; t < 9u; t++) {
    let tp = ((li.y + t / 3u) * 10u + li.x + (t % 3u)) * 16u;
    for (var iv = 0u; iv < 16u; iv++) {
      let v = tileM[tp + iv];
      let mb = P.matBase + (t * 16u + iv) * 16u + ovBase;
      for (var ov = 0u; ov < 8u; ov++) {
        acc[ov] += mats[mb + ov] * v;
      }
    }
  }
  let x = wg.x * 8u + li.x;
  let y = wg.y * 8u + li.y;
  if (x < P.width && y < P.height) {
    let base = (y * P.width + x) * 16u;
    for (var ov = 0u; ov < 8u; ov++) {
      dst[base + ovBase + ov] = prelu(acc[ov], vecs[P.slopeBase + ovBase + ov]);
    }
  }
}
