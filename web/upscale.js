// WebGL2 presenter: upscales the 320x200 RGBA frames the engine worker
// posts to the full canvas backing store with a selectable real-time
// filter. Falls back to the old 2d putImageData path (CSS nearest-neighbor
// scaling) when WebGL2 is unavailable.
//
// Filters:
//   xbr     — Hyllian's xBR-lv2 edge-interpolating scaler (MIT, adapted
//             from libretro/glsl-shaders xbr/shaders/xbr-lv2.glsl).
//             Best fit for the 8-bit era sprites and dithered backgrounds.
//   anime4k — bloc97's Anime4K 0.9 push algorithm (MIT): bilinear upscale,
//             luma push, then gradient-guided line sharpening.
//   off     — nearest-neighbor (original pixels).

"use strict";

const PAL_W = 320;
const PAL_H = 200;

const VERT_SRC = `#version 300 es
layout(location = 0) in vec2 aPos;
out vec2 vTex;
void main() {
  vTex = vec2(aPos.x * 0.5 + 0.5, 0.5 - aPos.y * 0.5);
  gl_Position = vec4(aPos, 0.0, 1.0);
}`;

// Plain textured quad; nearest/linear comes from the sampler state.
const COPY_FRAG = `#version 300 es
precision highp float;
uniform sampler2D uTex;
in vec2 vTex;
out vec4 FragColor;
void main() { FragColor = vec4(texture(uTex, vTex).rgb, 1.0); }`;

/*
   Hyllian's xBR-lv2 Shader — Copyright (C) 2011-2016 Hyllian
   (MIT license; adapted for GLSL ES 3.00, CORNER_C + SMOOTH_TIPS variant,
   texture offsets computed per-fragment, XBR_SCALE as a uniform.)
*/
const XBR_FRAG = `#version 300 es
precision highp float;
uniform sampler2D uTex;
uniform vec2 uTexSize;
uniform float uScale;
in vec2 vTex;
out vec4 FragColor;

#define XBR_EQ_THRESHOLD 15.0
#define XBR_LV2_COEFFICIENT 2.0

const vec3 rgbw = vec3(14.352, 28.176, 5.472);
const vec4 Ao = vec4( 1.0, -1.0, -1.0,  1.0);
const vec4 Bo = vec4( 1.0,  1.0, -1.0, -1.0);
const vec4 Co = vec4( 1.5,  0.5, -0.5,  0.5);
const vec4 Ax = vec4( 1.0, -1.0, -1.0,  1.0);
const vec4 Bx = vec4( 0.5,  2.0, -0.5, -2.0);
const vec4 Cx = vec4( 1.0,  1.0, -0.5,  0.0);
const vec4 Ay = vec4( 1.0, -1.0, -1.0,  1.0);
const vec4 By = vec4( 2.0,  0.5, -2.0, -0.5);
const vec4 Cy = vec4( 2.0,  0.0, -1.0,  0.5);
const vec4 Ci = vec4(0.25);

vec4 df(vec4 A, vec4 B) { return abs(A - B); }
vec4 diffv(vec4 A, vec4 B) { return vec4(notEqual(A, B)); }
vec4 eq(vec4 A, vec4 B) { return step(df(A, B), vec4(XBR_EQ_THRESHOLD)); }
vec4 neq(vec4 A, vec4 B) { return vec4(1.0) - eq(A, B); }
vec4 wd(vec4 a, vec4 b, vec4 c, vec4 d, vec4 e, vec4 f, vec4 g, vec4 h) {
  return df(a, b) + df(a, c) + df(d, e) + df(d, f) + 4.0 * df(g, h);
}
float c_df(vec3 c1, vec3 c2) { vec3 d = abs(c1 - c2); return d.r + d.g + d.b; }

void main() {
  vec4 delta   = vec4(1.0 / uScale);
  vec4 delta_l = vec4(0.5 / uScale, 1.0 / uScale, 0.5 / uScale, 1.0 / uScale);
  vec4 delta_u = delta_l.yxwz;

  vec2 tc = vTex * vec2(1.0000001, 1.0000001);
  vec2 fp = fract(tc * uTexSize);
  vec2 dx = vec2(1.0 / uTexSize.x, 0.0);
  vec2 dy = vec2(0.0, 1.0 / uTexSize.y);

  vec3 A1 = texture(uTex, tc - dx - 2.0 * dy).xyz;
  vec3 B1 = texture(uTex, tc      - 2.0 * dy).xyz;
  vec3 C1 = texture(uTex, tc + dx - 2.0 * dy).xyz;
  vec3 A  = texture(uTex, tc - dx - dy).xyz;
  vec3 B  = texture(uTex, tc      - dy).xyz;
  vec3 C  = texture(uTex, tc + dx - dy).xyz;
  vec3 D  = texture(uTex, tc - dx).xyz;
  vec3 E  = texture(uTex, tc).xyz;
  vec3 F  = texture(uTex, tc + dx).xyz;
  vec3 G  = texture(uTex, tc - dx + dy).xyz;
  vec3 H  = texture(uTex, tc      + dy).xyz;
  vec3 I  = texture(uTex, tc + dx + dy).xyz;
  vec3 G5 = texture(uTex, tc - dx + 2.0 * dy).xyz;
  vec3 H5 = texture(uTex, tc      + 2.0 * dy).xyz;
  vec3 I5 = texture(uTex, tc + dx + 2.0 * dy).xyz;
  vec3 A0 = texture(uTex, tc - 2.0 * dx - dy).xyz;
  vec3 D0 = texture(uTex, tc - 2.0 * dx).xyz;
  vec3 G0 = texture(uTex, tc - 2.0 * dx + dy).xyz;
  vec3 C4 = texture(uTex, tc + 2.0 * dx - dy).xyz;
  vec3 F4 = texture(uTex, tc + 2.0 * dx).xyz;
  vec3 I4 = texture(uTex, tc + 2.0 * dx + dy).xyz;

  vec4 b = vec4(dot(B, rgbw), dot(D, rgbw), dot(H, rgbw), dot(F, rgbw));
  vec4 c = vec4(dot(C, rgbw), dot(A, rgbw), dot(G, rgbw), dot(I, rgbw));
  vec4 d = b.yzwx;
  vec4 e = vec4(dot(E, rgbw));
  vec4 f = b.wxyz;
  vec4 g = c.zwxy;
  vec4 h = b.zwxy;
  vec4 i = c.wxyz;
  vec4 i4 = vec4(dot(I4, rgbw), dot(C1, rgbw), dot(A0, rgbw), dot(G5, rgbw));
  vec4 i5 = vec4(dot(I5, rgbw), dot(C4, rgbw), dot(A1, rgbw), dot(G0, rgbw));
  vec4 h5 = vec4(dot(H5, rgbw), dot(F4, rgbw), dot(B1, rgbw), dot(D0, rgbw));
  vec4 f4 = h5.yzwx;

  vec4 fx   = Ao * fp.y + Bo * fp.x;
  vec4 fx_l = Ax * fp.y + Bx * fp.x;
  vec4 fx_u = Ay * fp.y + By * fp.x;

  vec4 irlv0 = diffv(e, f) * diffv(e, h);
  // CORNER_C corner detection.
  vec4 irlv1 = irlv0 * (neq(f, b) * neq(f, c) + neq(h, d) * neq(h, g)
      + eq(e, i) * (neq(f, f4) * neq(f, i4) + neq(h, h5) * neq(h, i5))
      + eq(e, g) + eq(e, c));
  vec4 irlv2l = diffv(e, g) * diffv(d, g);
  vec4 irlv2u = diffv(e, c) * diffv(b, c);

  vec4 fx45i = clamp((fx   + delta   - Co - Ci) / (2.0 * delta),   0.0, 1.0);
  vec4 fx45  = clamp((fx   + delta   - Co)      / (2.0 * delta),   0.0, 1.0);
  vec4 fx30  = clamp((fx_l + delta_l - Cx)      / (2.0 * delta_l), 0.0, 1.0);
  vec4 fx60  = clamp((fx_u + delta_u - Cy)      / (2.0 * delta_u), 0.0, 1.0);

  vec4 wd1 = wd(e, c, g, i, h5, f4, h, f);
  vec4 wd2 = wd(h, d, i5, f, i4, b, e, i);

  vec4 edri  = step(wd1, wd2) * irlv0;
  vec4 edr   = step(wd1 + vec4(0.1), wd2) * step(vec4(0.5), irlv1);
  vec4 edr_l = step(XBR_LV2_COEFFICIENT * df(f, g), df(h, c)) * irlv2l * edr;
  vec4 edr_u = step(XBR_LV2_COEFFICIENT * df(h, c), df(f, g)) * irlv2u * edr;

  fx45  = edr   * fx45;
  fx30  = edr_l * fx30;
  fx60  = edr_u * fx60;
  fx45i = edri  * fx45i;

  vec4 px = step(df(e, f), df(e, h));
  vec4 maximos = max(max(fx30, fx60), max(fx45, fx45i));

  vec3 res1 = E;
  res1 = mix(res1, mix(H, F, px.x), maximos.x);
  res1 = mix(res1, mix(B, D, px.z), maximos.z);

  vec3 res2 = E;
  res2 = mix(res2, mix(F, B, px.y), maximos.y);
  res2 = mix(res2, mix(D, H, px.w), maximos.w);

  vec3 res = mix(res1, res2, step(c_df(E, res1), c_df(E, res2)));
  FragColor = vec4(res, 1.0);
}`;

// Anime4K 0.9 (bloc97, MIT) pass 1: bilinear upscale (linear sampler at
// output resolution) storing luma in alpha for the push passes.
const A4K_LUMA_FRAG = `#version 300 es
precision highp float;
uniform sampler2D uTex;
in vec2 vTex;
out vec4 FragColor;
void main() {
  vec3 c = texture(uTex, vTex).rgb;
  FragColor = vec4(c, dot(c, vec3(0.299, 0.587, 0.114)));
}`;

// Anime4K push kernel, shared by the color-push and gradient-push passes:
// for 8 directional kernels, if one side of the pixel is strictly
// "lighter" (higher alpha) than the other, blend toward the light side.
// In the color pass alpha holds luma; in the gradient pass it holds the
// inverted gradient, so blending pulls flat-region color over edge pixels
// (line thinning / sharpening). FINAL differs: the color pass keeps the
// blended alpha for the following Sobel pass, the gradient pass emits
// opaque pixels for display.
function a4kPushFrag(final) {
  return `#version 300 es
precision highp float;
uniform sampler2D uTex;
uniform vec2 uPx;
uniform float uStrength;
in vec2 vTex;
out vec4 FragColor;

float max3v(float a, float b, float c) { return max(a, max(b, c)); }
float min3v(float a, float b, float c) { return min(a, min(b, c)); }

vec4 getLargest(vec4 cc, vec4 lightest, vec4 a, vec4 b, vec4 c) {
  vec4 n = cc * (1.0 - uStrength) + ((a + b + c) / 3.0) * uStrength;
  return n.a > lightest.a ? n : lightest;
}

void main() {
  vec4 tl = texture(uTex, vTex + uPx * vec2(-1.0, -1.0));
  vec4 tc = texture(uTex, vTex + uPx * vec2( 0.0, -1.0));
  vec4 tr = texture(uTex, vTex + uPx * vec2( 1.0, -1.0));
  vec4 ml = texture(uTex, vTex + uPx * vec2(-1.0,  0.0));
  vec4 mc = texture(uTex, vTex);
  vec4 mr = texture(uTex, vTex + uPx * vec2( 1.0,  0.0));
  vec4 bl = texture(uTex, vTex + uPx * vec2(-1.0,  1.0));
  vec4 bc = texture(uTex, vTex + uPx * vec2( 0.0,  1.0));
  vec4 br = texture(uTex, vTex + uPx * vec2( 1.0,  1.0));

  vec4 lightest = mc;
  float maxDark, minLight;

  // Kernel 0+4: top row vs bottom row.
  maxDark = max3v(br.a, bc.a, bl.a);
  minLight = min3v(tl.a, tc.a, tr.a);
  if (minLight > mc.a && minLight > maxDark) {
    lightest = getLargest(mc, lightest, tl, tc, tr);
  } else {
    maxDark = max3v(tl.a, tc.a, tr.a);
    minLight = min3v(br.a, bc.a, bl.a);
    if (minLight > mc.a && minLight > maxDark) {
      lightest = getLargest(mc, lightest, br, bc, bl);
    }
  }

  // Kernel 1+5: top-right corner vs bottom-left corner.
  maxDark = max3v(mc.a, ml.a, bc.a);
  minLight = min3v(mr.a, tc.a, tr.a);
  if (minLight > maxDark) {
    lightest = getLargest(mc, lightest, mr, tc, tr);
  } else {
    maxDark = max3v(mc.a, mr.a, tc.a);
    minLight = min3v(bl.a, ml.a, bc.a);
    if (minLight > maxDark) {
      lightest = getLargest(mc, lightest, bl, ml, bc);
    }
  }

  // Kernel 2+6: right column vs left column.
  maxDark = max3v(ml.a, tl.a, bl.a);
  minLight = min3v(mr.a, br.a, tr.a);
  if (minLight > mc.a && minLight > maxDark) {
    lightest = getLargest(mc, lightest, mr, br, tr);
  } else {
    maxDark = max3v(mr.a, br.a, tr.a);
    minLight = min3v(ml.a, tl.a, bl.a);
    if (minLight > mc.a && minLight > maxDark) {
      lightest = getLargest(mc, lightest, ml, tl, bl);
    }
  }

  // Kernel 3+7: bottom-right corner vs top-left corner.
  maxDark = max3v(mc.a, ml.a, tc.a);
  minLight = min3v(mr.a, br.a, bc.a);
  if (minLight > maxDark) {
    lightest = getLargest(mc, lightest, mr, br, bc);
  } else {
    maxDark = max3v(mc.a, mr.a, bc.a);
    minLight = min3v(tl.a, ml.a, tc.a);
    if (minLight > maxDark) {
      lightest = getLargest(mc, lightest, tl, ml, tc);
    }
  }

  ${final}
}`;
}

// Anime4K pass 3: Sobel on the luma alpha, storing the inverted gradient
// magnitude in alpha (1 = flat, 0 = edge) for the gradient push.
const A4K_GRAD_FRAG = `#version 300 es
precision highp float;
uniform sampler2D uTex;
uniform vec2 uPx;
in vec2 vTex;
out vec4 FragColor;
void main() {
  float tl = texture(uTex, vTex + uPx * vec2(-1.0, -1.0)).a;
  float tc = texture(uTex, vTex + uPx * vec2( 0.0, -1.0)).a;
  float tr = texture(uTex, vTex + uPx * vec2( 1.0, -1.0)).a;
  float ml = texture(uTex, vTex + uPx * vec2(-1.0,  0.0)).a;
  float mr = texture(uTex, vTex + uPx * vec2( 1.0,  0.0)).a;
  float bl = texture(uTex, vTex + uPx * vec2(-1.0,  1.0)).a;
  float bc = texture(uTex, vTex + uPx * vec2( 0.0,  1.0)).a;
  float br = texture(uTex, vTex + uPx * vec2( 1.0,  1.0)).a;
  float xg = -tl - 2.0 * ml - bl + tr + 2.0 * mr + br;
  float yg = -tl - 2.0 * tc - tr + bl + 2.0 * bc + br;
  vec3 rgb = texture(uTex, vTex).rgb;
  FragColor = vec4(rgb, 1.0 - clamp(sqrt(xg * xg + yg * yg), 0.0, 1.0));
}`;

class PalPresenter {
  constructor(canvas) {
    this.canvas = canvas;
    this.filter = localStorage.getItem("pal-filter") || "xbr";
    this.lastFrame = null;
    this.needResize = true;
    this.gl = canvas.getContext("webgl2", { alpha: false, antialias: false });
    if (this.gl) {
      canvas.style.imageRendering = "auto";
      canvas.addEventListener("webglcontextlost", (e) => {
        e.preventDefault();
        this.dead = true;
      });
      canvas.addEventListener("webglcontextrestored", () => {
        this.dead = false;
        this.initGL();
        if (this.lastFrame) this.present(this.lastFrame);
      });
      this.initGL();
      new ResizeObserver(() => {
        this.needResize = true;
        if (this.lastFrame) this.present(this.lastFrame);
      }).observe(canvas);
    } else {
      // 2d fallback: original fixed-size canvas, CSS does the scaling.
      canvas.width = PAL_W;
      canvas.height = PAL_H;
      this.ctx2d = canvas.getContext("2d");
    }
  }

  initGL() {
    const gl = this.gl;
    const compile = (type, src) => {
      const s = gl.createShader(type);
      gl.shaderSource(s, src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
        throw new Error(`shader: ${gl.getShaderInfoLog(s)}`);
      }
      return s;
    };
    const vert = compile(gl.VERTEX_SHADER, VERT_SRC);
    const link = (fragSrc) => {
      const p = gl.createProgram();
      gl.attachShader(p, vert);
      gl.attachShader(p, compile(gl.FRAGMENT_SHADER, fragSrc));
      gl.linkProgram(p);
      if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
        throw new Error(`link: ${gl.getProgramInfoLog(p)}`);
      }
      return p;
    };
    this.progCopy = link(COPY_FRAG);
    this.progXbr = link(XBR_FRAG);
    this.progLuma = link(A4K_LUMA_FRAG);
    this.progPush = link(a4kPushFrag("FragColor = lightest;"));
    this.progGrad = link(A4K_GRAD_FRAG);
    this.progPushGrad = link(a4kPushFrag("FragColor = vec4(lightest.rgb, 1.0);"));

    // Fullscreen triangle.
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    const vbo = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
    gl.bufferData(gl.ARRAY_BUFFER,
      new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

    const makeTex = (w, h) => {
      const t = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, t);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0,
        gl.RGBA, gl.UNSIGNED_BYTE, null);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      return t;
    };
    this.srcTex = makeTex(PAL_W, PAL_H);
    this.texA = makeTex(1, 1); // sized on first resize
    this.texB = makeTex(1, 1);
    this.fboA = gl.createFramebuffer();
    this.fboB = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.fboA);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0,
      gl.TEXTURE_2D, this.texA, 0);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.fboB);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0,
      gl.TEXTURE_2D, this.texB, 0);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    this.needResize = true;
  }

  setFilter(f) {
    this.filter = f;
    localStorage.setItem("pal-filter", f);
    if (this.lastFrame) this.present(this.lastFrame);
  }

  resize() {
    const gl = this.gl;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(PAL_W, Math.round(this.canvas.clientWidth * dpr));
    const h = Math.max(PAL_H, Math.round(this.canvas.clientHeight * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    for (const t of [this.texA, this.texB]) {
      gl.bindTexture(gl.TEXTURE_2D, t);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0,
        gl.RGBA, gl.UNSIGNED_BYTE, null);
    }
    this.needResize = false;
  }

  present(pixels) {
    this.lastFrame = pixels;
    if (this.ctx2d) {
      this.ctx2d.putImageData(
        new ImageData(new Uint8ClampedArray(pixels.buffer, pixels.byteOffset,
          pixels.length), PAL_W, PAL_H), 0, 0);
      return;
    }
    if (this.dead) return;
    const gl = this.gl;
    if (this.needResize) this.resize();
    const w = this.canvas.width;
    const h = this.canvas.height;

    const srcFilter = this.filter === "anime4k" ? gl.LINEAR : gl.NEAREST;
    gl.bindTexture(gl.TEXTURE_2D, this.srcTex);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, srcFilter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, srcFilter);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, PAL_W, PAL_H,
      gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    gl.viewport(0, 0, w, h);
    gl.activeTexture(gl.TEXTURE0);

    const draw = (prog, tex, fbo, uniforms) => {
      gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
      gl.useProgram(prog);
      gl.bindTexture(gl.TEXTURE_2D, tex);
      gl.uniform1i(gl.getUniformLocation(prog, "uTex"), 0);
      if (uniforms) uniforms(prog);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };

    if (this.filter === "xbr") {
      draw(this.progXbr, this.srcTex, null, (p) => {
        gl.uniform2f(gl.getUniformLocation(p, "uTexSize"), PAL_W, PAL_H);
        gl.uniform1f(gl.getUniformLocation(p, "uScale"),
          Math.max(2, h / PAL_H));
      });
    } else if (this.filter === "anime4k") {
      const scale = h / PAL_H;
      const pushStrength = Math.min(scale / 6, 1);
      const gradStrength = Math.min(scale / 2, 1);
      const px = (p) => gl.uniform2f(gl.getUniformLocation(p, "uPx"), 1 / w, 1 / h);
      draw(this.progLuma, this.srcTex, this.fboA);
      draw(this.progPush, this.texA, this.fboB, (p) => {
        px(p);
        gl.uniform1f(gl.getUniformLocation(p, "uStrength"), pushStrength);
      });
      draw(this.progGrad, this.texB, this.fboA, px);
      draw(this.progPushGrad, this.texA, null, (p) => {
        px(p);
        gl.uniform1f(gl.getUniformLocation(p, "uStrength"), gradStrength);
      });
    } else {
      draw(this.progCopy, this.srcTex, null);
    }
  }
}
