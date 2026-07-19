struct VertexOutput {
  @builtin(position) position : vec4<f32>,
  @location(0) uv : vec2<f32>,
};

@group(0) @binding(0) var image : texture_2d<f32>;
@group(0) @binding(1) var imageSampler : sampler;

@vertex
fn vs(@builtin(vertex_index) index : u32) -> VertexOutput {
  let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
  var output : VertexOutput;
  output.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0),
                              0.0, 1.0);
  output.uv = uv;
  return output;
}

fn srgbToLinearPart(c : f32) -> f32 {
  if (c <= 0.04045) {
    return c / 12.92;
  }
  return pow((c + 0.055) / 1.055, 2.4);
}

@fragment
fn fs(input : VertexOutput) -> @location(0) vec4<f32> {
  let encoded = textureSample(image, imageSampler, input.uv).rgb;
  // The neural texture stores encoded sRGB values in rgba8unorm. Convert to
  // linear before writing to the native sRGB swapchain so presentation does
  // not apply the transfer function twice.
  let linear = vec3<f32>(srgbToLinearPart(encoded.r),
                         srgbToLinearPart(encoded.g),
                         srgbToLinearPart(encoded.b));
  return vec4<f32>(linear, 1.0);
}
