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

// Exact 8x8x4, two-pixel tuned kernel from web/nn-upscale.js. Each weight
// fetch feeds two pixels and the 18x10 halo tile uses 23,040 bytes.
var<workgroup> tileM : array<vec4<f16>, 2880>;

fn prelu(a : vec4<f16>, s : vec4<f16>) -> vec4<f16> {
  return max(a, vec4<f16>()) + s * min(a, vec4<f16>());
}

@compute @workgroup_size(8, 8, 4)
fn conv_mid(@builtin(workgroup_id) wg : vec3<u32>,
            @builtin(local_invocation_id) li : vec3<u32>,
            @builtin(local_invocation_index) lidx : u32) {
  let iw = i32(P.width);
  let ih = i32(P.height);
  let x0 = i32(wg.x * 16u) - 1;
  let y0 = i32(wg.y * 8u) - 1;
  for (var p = lidx; p < 180u; p += 256u) {
    let pxx = x0 + i32(p % 18u);
    let pyy = y0 + i32(p / 18u);
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

  let ovBase = li.z * 4u;
  var acc0 : array<vec4<f16>, 4>;
  var acc1 : array<vec4<f16>, 4>;
  for (var ov = 0u; ov < 4u; ov++) {
    let bias = vecs[P.biasBase + ovBase + ov];
    acc0[ov] = bias;
    acc1[ov] = bias;
  }
  for (var t = 0u; t < 9u; t++) {
    let tp = ((li.y + t / 3u) * 18u + li.x * 2u + (t % 3u)) * 16u;
    for (var iv = 0u; iv < 16u; iv++) {
      let v0 = tileM[tp + iv];
      let v1 = tileM[tp + 16u + iv];
      let mb = P.matBase + (t * 16u + iv) * 16u + ovBase;
      for (var ov = 0u; ov < 4u; ov++) {
        let mat = mats[mb + ov];
        acc0[ov] += mat * v0;
        acc1[ov] += mat * v1;
      }
    }
  }
  let y = wg.y * 8u + li.y;
  if (y >= P.height) {
    return;
  }
  let x0Out = wg.x * 16u + li.x * 2u;
  for (var ov = 0u; ov < 4u; ov++) {
    let slope = vecs[P.slopeBase + ovBase + ov];
    if (x0Out < P.width) {
      let base = (y * P.width + x0Out) * 16u;
      dst[base + ovBase + ov] = prelu(acc0[ov], slope);
    }
    if (x0Out + 1u < P.width) {
      let base = (y * P.width + x0Out + 1u) * 16u;
      dst[base + ovBase + ov] = prelu(acc1[ov], slope);
    }
  }
}
