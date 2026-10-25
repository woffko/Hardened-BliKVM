// The upscalers compared in phase 0 of docs/baselines/work-plan-20261008-screen-upscaling.md, as WebGL2 programs.
// A lab tool, never shipped. Ported algorithms keep their authors' notices:
//
// - SGSR 1 (sgsr1_shader_mobile.frag): Copyright (c) 2023-2025, Qualcomm Innovation Center, Inc. All rights
//   reserved. SPDX-License-Identifier: BSD-3-Clause. Ported to WebGL2: textureGather replaced by four texelFetch.
// - FSR 1 EASU and RCAS (ffx_fsr1.h, v1.20210629): Copyright (c) 2021 Advanced Micro Devices, Inc. All rights
//   reserved. MIT licence. Ported to WebGL2 GLSL: the twelve taps fetched directly, the 32-bit path only.
// - Anime4K (Anime4K_Upscale_CNN_x2_S/M.glsl): Copyright (c) 2019-2021 bloc97. MIT licence. Loaded from the
//   upstream file and translated pass by pass at run time.
//
// The licence texts, the upstream commits and the files' SHA-256 are in THIRD-PARTY-NOTICES.txt beside this file.
//
// Conventions: the picture's row 0 is the top row, both in the source texture and in every render target, so a
// framebuffer read starts with the top row. A fragment's output pixel is gl_FragCoord.xy, its center in source
// pixels (centres at k + 0.5) is gl_FragCoord.xy * src / dst.

const VERTEX = `#version 300 es
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;

const HEADER = `#version 300 es
precision highp float;
precision highp int;
uniform sampler2D uSrc;
uniform vec2 uSrcSize;
uniform vec2 uDstSize;
out vec4 outColor;
vec4 tap(ivec2 p) { return texelFetch(uSrc, clamp(p, ivec2(0), ivec2(uSrcSize) - 1), 0); }
// Source position of this output pixel, with pixel centres at integers.
vec2 srcPos() { return gl_FragCoord.xy * (uSrcSize / uDstSize) - 0.5; }
`;

const BILINEAR = `${HEADER}
void main() { outColor = vec4(texture(uSrc, gl_FragCoord.xy / uDstSize).rgb, 1.0); }`;

const BICUBIC = `${HEADER}
vec4 weights(float t) {
  float t2 = t * t, t3 = t2 * t;
  return vec4(-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0, -1.5 * t3 + 2.0 * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2);
}
void main() {
  vec2 p = srcPos(), f = floor(p), t = p - f;
  vec4 wx = weights(t.x), wy = weights(t.y);
  ivec2 b = ivec2(f);
  vec3 acc = vec3(0.0);
  for (int j = 0; j < 4; j++) {
    vec3 row = vec3(0.0);
    for (int i = 0; i < 4; i++) row += tap(b + ivec2(i - 1, j - 1)).rgb * wx[i];
    acc += row * wy[j];
  }
  outColor = vec4(clamp(acc, 0.0, 1.0), 1.0);
}`;

const LANCZOS2 = `${HEADER}
float l2(float x) {
  x = abs(x);
  if (x < 1e-5) return 1.0;
  if (x >= 2.0) return 0.0;
  float px = 3.14159265 * x;
  return 2.0 * sin(px) * sin(px * 0.5) / (px * px);
}
void main() {
  vec2 p = srcPos(), f = floor(p), t = p - f;
  vec4 wx = vec4(l2(-1.0 - t.x), l2(-t.x), l2(1.0 - t.x), l2(2.0 - t.x));
  vec4 wy = vec4(l2(-1.0 - t.y), l2(-t.y), l2(1.0 - t.y), l2(2.0 - t.y));
  wx /= dot(wx, vec4(1.0));
  wy /= dot(wy, vec4(1.0));
  ivec2 b = ivec2(f);
  vec3 acc = vec3(0.0);
  for (int j = 0; j < 4; j++) {
    vec3 row = vec3(0.0);
    for (int i = 0; i < 4; i++) row += tap(b + ivec2(i - 1, j - 1)).rgb * wx[i];
    acc += row * wy[j];
  }
  outColor = vec4(clamp(acc, 0.0, 1.0), 1.0);
}`;

// Catmull-Rom stretched by the reduction, for pictures made larger than wanted (Anime4K's 2x to 4/3).
const DOWNSCALE = `${HEADER}
float cr(float x) {
  x = abs(x);
  if (x < 1.0) return 1.5 * x * x * x - 2.5 * x * x + 1.0;
  if (x < 2.0) return -0.5 * x * x * x + 2.5 * x * x - 4.0 * x + 2.0;
  return 0.0;
}
void main() {
  vec2 ratio = uSrcSize / uDstSize;
  vec2 c = srcPos();
  ivec2 lo = ivec2(ceil(c - 2.0 * ratio)), hi = ivec2(floor(c + 2.0 * ratio));
  vec3 acc = vec3(0.0);
  float sum = 0.0;
  for (int y = lo.y; y <= hi.y; y++) {
    float wy = cr((float(y) - c.y) / ratio.y);
    for (int x = lo.x; x <= hi.x; x++) {
      float w = wy * cr((float(x) - c.x) / ratio.x);
      acc += tap(ivec2(x, y)).rgb * w;
      sum += w;
    }
  }
  outColor = vec4(clamp(acc / sum, 0.0, 1.0), 1.0);
}`;

// A deringing of the source before it is enlarged: a 5x5 bilateral filter whose range weight comes from the luma
// difference (sigma uSigma), so that the JPEG's ringing beside text (a few levels) is smoothed and the text's own
// edges (a hundred levels and more) are kept.
const DERING = `${HEADER}
uniform float uSigma;
void main() {
  ivec2 p = ivec2(gl_FragCoord.xy);
  const vec3 toLuma = vec3(0.299, 0.587, 0.114);
  float centre = dot(tap(p).rgb, toLuma);
  vec3 acc = vec3(0.0);
  float sum = 0.0;
  for (int j = -2; j <= 2; j++) {
    for (int i = -2; i <= 2; i++) {
      vec3 c = tap(p + ivec2(i, j)).rgb;
      float d = dot(c, toLuma) - centre;
      float w = exp(-float(i * i + j * j) * (1.0 / 4.5) - d * d / (2.0 * uSigma * uSigma));
      acc += c * w;
      sum += w;
    }
  }
  outColor = vec4(acc / sum, 1.0);
}`;

// A plain sharpening of the source before it is enlarged: c + k (4c - n - s - e - w).
const SHARPEN = `${HEADER}
uniform float uK;
void main() {
  ivec2 p = ivec2(gl_FragCoord.xy);
  vec3 c = tap(p).rgb;
  vec3 n = tap(p + ivec2(0, -1)).rgb + tap(p + ivec2(0, 1)).rgb + tap(p + ivec2(-1, 0)).rgb + tap(p + ivec2(1, 0)).rgb;
  outColor = vec4(clamp(c + uK * (4.0 * c - n), 0.0, 1.0), 1.0);
}`;

// SGSR 1, OperationMode 1 (RGBA, green as the luma), EdgeThreshold 8/255, EdgeSharpness uEdge (2.0 upstream).
const SGSR = `${HEADER}
float fastLanczos2(float x) {
  float wA = x - 4.0;
  float wB = x * wA - wA;
  wA *= wA;
  return wB * wA;
}
vec2 weightY(float dx, float dy, float c, float stdv) {
  float x = ((dx * dx) + (dy * dy)) * 0.55 + clamp(abs(c) * stdv, 0.0, 1.0);
  float w = fastLanczos2(x);
  return vec2(w, w * c);
}
// textureGather(.., at the texel corner 'corner', green): x = (i0, j1), y = (i1, j1), z = (i1, j0), w = (i0, j0).
vec4 gatherG(ivec2 corner) {
  ivec2 p = corner - 1;
  return vec4(tap(p + ivec2(0, 1)).g, tap(p + ivec2(1, 1)).g, tap(p + ivec2(1, 0)).g, tap(p).g);
}
uniform float uEdge;
void main() {
  float edgeThreshold = 8.0 / 255.0;
  float edgeSharpness = uEdge;
  vec2 uv = gl_FragCoord.xy / uDstSize;
  vec4 color = vec4(texture(uSrc, uv).rgb, 0.0);
  vec2 imgCoord = uv * uSrcSize + vec2(-0.5, 0.5);
  vec2 imgCoordPixel = floor(imgCoord);
  ivec2 corner = ivec2(imgCoordPixel);
  vec2 pl = imgCoord - imgCoordPixel;
  vec4 left = gatherG(corner);
  float edgeVote = abs(left.z - left.y) + abs(color.g - left.y) + abs(color.g - left.z);
  if (edgeVote > edgeThreshold) {
    corner.x += 1;
    vec4 right = gatherG(corner + ivec2(1, 0));
    vec4 upDown;
    upDown.xy = gatherG(corner + ivec2(0, -1)).wz;
    upDown.zw = gatherG(corner + ivec2(0, 1)).yx;
    float mean = (left.y + left.z + right.x + right.w) * 0.25;
    left = left - vec4(mean);
    right = right - vec4(mean);
    upDown = upDown - vec4(mean);
    color.w = color.g - mean;
    float sum = (((((abs(left.x) + abs(left.y)) + abs(left.z)) + abs(left.w)) + (((abs(right.x) + abs(right.y)) + abs(right.z)) + abs(right.w))) + (((abs(upDown.x) + abs(upDown.y)) + abs(upDown.z)) + abs(upDown.w)));
    float stdv = 2.181818 / sum;
    vec2 aWY = weightY(pl.x, pl.y + 1.0, upDown.x, stdv);
    aWY += weightY(pl.x - 1.0, pl.y + 1.0, upDown.y, stdv);
    aWY += weightY(pl.x - 1.0, pl.y - 2.0, upDown.z, stdv);
    aWY += weightY(pl.x, pl.y - 2.0, upDown.w, stdv);
    aWY += weightY(pl.x + 1.0, pl.y - 1.0, left.x, stdv);
    aWY += weightY(pl.x, pl.y - 1.0, left.y, stdv);
    aWY += weightY(pl.x, pl.y, left.z, stdv);
    aWY += weightY(pl.x + 1.0, pl.y, left.w, stdv);
    aWY += weightY(pl.x - 1.0, pl.y - 1.0, right.x, stdv);
    aWY += weightY(pl.x - 2.0, pl.y - 1.0, right.y, stdv);
    aWY += weightY(pl.x - 2.0, pl.y, right.z, stdv);
    aWY += weightY(pl.x - 1.0, pl.y, right.w, stdv);
    float finalY = aWY.y / aWY.x;
    float maxY = max(max(left.y, left.z), max(right.x, right.w));
    float minY = min(min(left.y, left.z), min(right.x, right.w));
    finalY = clamp(edgeSharpness * finalY, minY, maxY);
    float deltaY = finalY - color.w;
    deltaY = clamp(deltaY, -23.0 / 255.0, 23.0 / 255.0);
    color.rgb = clamp(color.rgb + deltaY, 0.0, 1.0);
  }
  outColor = vec4(color.rgb, 1.0);
}`;

// FSR 1 EASU, 32-bit path, with AMD's approximations kept.
const FSR_EASU = `${HEADER}
float prxLoRcp(float a) { return uintBitsToFloat(0x7ef07ebbu - floatBitsToUint(a)); }
float prxLoRsq(float a) { return uintBitsToFloat(0x5f347d74u - (floatBitsToUint(a) >> 1u)); }
void easuTap(inout vec3 aC, inout float aW, vec2 off, vec2 dir, vec2 len, float lob, float clp, vec3 c) {
  vec2 v;
  v.x = (off.x * (dir.x)) + (off.y * dir.y);
  v.y = (off.x * (-dir.y)) + (off.y * dir.x);
  v *= len;
  float d2 = v.x * v.x + v.y * v.y;
  d2 = min(d2, clp);
  float wB = (2.0 / 5.0) * d2 - 1.0;
  float wA = lob * d2 - 1.0;
  wB *= wB;
  wA *= wA;
  wB = (25.0 / 16.0) * wB - (25.0 / 16.0 - 1.0);
  float w = wB * wA;
  aC += c * w;
  aW += w;
}
void easuSet(inout vec2 dir, inout float len, vec2 pp, int corner, float lA, float lB, float lC, float lD, float lE) {
  float w = 0.0;
  if (corner == 0) w = (1.0 - pp.x) * (1.0 - pp.y);
  if (corner == 1) w = pp.x * (1.0 - pp.y);
  if (corner == 2) w = (1.0 - pp.x) * pp.y;
  if (corner == 3) w = pp.x * pp.y;
  float dc = lD - lC;
  float cb = lC - lB;
  float lenX = max(abs(dc), abs(cb));
  lenX = prxLoRcp(lenX);
  float dirX = lD - lB;
  dir.x += dirX * w;
  lenX = clamp(abs(dirX) * lenX, 0.0, 1.0);
  lenX *= lenX;
  len += lenX * w;
  float ec = lE - lC;
  float ca = lC - lA;
  float lenY = max(abs(ec), abs(ca));
  lenY = prxLoRcp(lenY);
  float dirY = lE - lA;
  dir.y += dirY * w;
  lenY = clamp(abs(dirY) * lenY, 0.0, 1.0);
  lenY *= lenY;
  len += lenY * w;
}
float L(vec3 c) { return c.b * 0.5 + (c.r * 0.5 + c.g); }
void main() {
  vec2 pp = srcPos();
  vec2 fp = floor(pp);
  pp -= fp;
  ivec2 f0 = ivec2(fp);
  //    b c
  //  e f g h
  //  i j k l
  //    n o
  vec3 b = tap(f0 + ivec2(0, -1)).rgb, c = tap(f0 + ivec2(1, -1)).rgb;
  vec3 e = tap(f0 + ivec2(-1, 0)).rgb, f = tap(f0).rgb, g = tap(f0 + ivec2(1, 0)).rgb, h = tap(f0 + ivec2(2, 0)).rgb;
  vec3 i = tap(f0 + ivec2(-1, 1)).rgb, j = tap(f0 + ivec2(0, 1)).rgb, k = tap(f0 + ivec2(1, 1)).rgb, l = tap(f0 + ivec2(2, 1)).rgb;
  vec3 n = tap(f0 + ivec2(0, 2)).rgb, o = tap(f0 + ivec2(1, 2)).rgb;
  float bL = L(b), cL = L(c), eL = L(e), fL = L(f), gL = L(g), hL = L(h), iL = L(i), jL = L(j), kL = L(k), lL = L(l), nL = L(n), oL = L(o);
  vec2 dir = vec2(0.0);
  float len = 0.0;
  easuSet(dir, len, pp, 0, bL, eL, fL, gL, jL);
  easuSet(dir, len, pp, 1, cL, fL, gL, hL, kL);
  easuSet(dir, len, pp, 2, fL, iL, jL, kL, nL);
  easuSet(dir, len, pp, 3, gL, jL, kL, lL, oL);
  vec2 dir2 = dir * dir;
  float dirR = dir2.x + dir2.y;
  bool zro = dirR < (1.0 / 32768.0);
  dirR = prxLoRsq(dirR);
  dirR = zro ? 1.0 : dirR;
  dir.x = zro ? 1.0 : dir.x;
  dir *= vec2(dirR);
  len = len * 0.5;
  len *= len;
  float stretch = (dir.x * dir.x + dir.y * dir.y) * prxLoRcp(max(abs(dir.x), abs(dir.y)));
  vec2 len2 = vec2(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
  float lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
  float clp = prxLoRcp(lob);
  vec3 min4 = min(min(min(f, g), j), k);
  vec3 max4 = max(max(max(f, g), j), k);
  vec3 aC = vec3(0.0);
  float aW = 0.0;
  easuTap(aC, aW, vec2(0.0, -1.0) - pp, dir, len2, lob, clp, b);
  easuTap(aC, aW, vec2(1.0, -1.0) - pp, dir, len2, lob, clp, c);
  easuTap(aC, aW, vec2(-1.0, 1.0) - pp, dir, len2, lob, clp, i);
  easuTap(aC, aW, vec2(0.0, 1.0) - pp, dir, len2, lob, clp, j);
  easuTap(aC, aW, vec2(0.0, 0.0) - pp, dir, len2, lob, clp, f);
  easuTap(aC, aW, vec2(-1.0, 0.0) - pp, dir, len2, lob, clp, e);
  easuTap(aC, aW, vec2(1.0, 1.0) - pp, dir, len2, lob, clp, k);
  easuTap(aC, aW, vec2(2.0, 1.0) - pp, dir, len2, lob, clp, l);
  easuTap(aC, aW, vec2(2.0, 0.0) - pp, dir, len2, lob, clp, h);
  easuTap(aC, aW, vec2(1.0, 0.0) - pp, dir, len2, lob, clp, g);
  easuTap(aC, aW, vec2(1.0, 2.0) - pp, dir, len2, lob, clp, o);
  easuTap(aC, aW, vec2(0.0, 2.0) - pp, dir, len2, lob, clp, n);
  outColor = vec4(min(max4, max(min4, aC / aW)), 1.0);
}`;

// FSR 1 RCAS on a picture already at the output size (uSrc), sharpness in stops (0 = most).
const FSR_RCAS = `${HEADER}
uniform float uSharp;
float prxMedRcp(float a) { float b = uintBitsToFloat(0x7ef19fffu - floatBitsToUint(a)); return b * (-b * a + 2.0); }
void main() {
  ivec2 sp = ivec2(gl_FragCoord.xy);
  vec3 b = tap(sp + ivec2(0, -1)).rgb, d = tap(sp + ivec2(-1, 0)).rgb, e = tap(sp).rgb;
  vec3 f = tap(sp + ivec2(1, 0)).rgb, h = tap(sp + ivec2(0, 1)).rgb;
  vec3 mn4 = min(min(min(b, d), f), h);
  vec3 mx4 = max(max(max(b, d), f), h);
  vec2 peakC = vec2(1.0, -4.0);
  vec3 hitMin = min(mn4, e) / (4.0 * mx4);
  vec3 hitMax = (peakC.x - max(mx4, e)) / (4.0 * mn4 + peakC.y);
  vec3 lobeRGB = max(-hitMin, hitMax);
  float lobe = max(-(0.25 - (1.0 / 16.0)), min(max(max(lobeRGB.r, lobeRGB.g), lobeRGB.b), 0.0)) * exp2(-uSharp);
  float rcpL = prxMedRcp(4.0 * lobe + 1.0);
  outColor = vec4((lobe * b + lobe * d + lobe * h + lobe * f + e) * rcpL, 1.0);
}`;

/**
 * Translates an Anime4K mpv user shader (the CNN upscalers: same-size convolutions, then depth to space at twice the
 * size) into WebGL2 passes: [{ binds, save, scale, source }].
 */
export function translateAnime4K(text) {
  const passes = [];
  for (const chunk of text.split(/^(?=\/\/!DESC)/m).filter((c) => c.startsWith("//!DESC"))) {
    const lines = chunk.split("\n");
    const binds = lines.filter((l) => l.startsWith("//!BIND ")).map((l) => l.slice(8).trim());
    const save = (lines.find((l) => l.startsWith("//!SAVE ")) ?? "//!SAVE MAIN").slice(8).trim();
    const width = (lines.find((l) => l.startsWith("//!WIDTH ")) ?? "").slice(9).trim();
    const scale = /\s2 \*$/.test(width) ? 2 : 1;
    const body = lines.filter((l) => !l.startsWith("//!")).join("\n");
    let helpers = "";
    for (const name of binds) {
      helpers += `uniform sampler2D ${name}_raw;\n`;
      helpers += `vec4 ${name}_texOff(vec2 off) { ivec2 size = textureSize(${name}_raw, 0); return texelFetch(${name}_raw, clamp(ivec2(gl_FragCoord.xy) + ivec2(off), ivec2(0), size - 1), 0); }\n`;
      helpers += `#define ${name}_pos (gl_FragCoord.xy / uDstSize)\n`;
      helpers += `#define ${name}_size vec2(textureSize(${name}_raw, 0))\n`;
      helpers += `#define ${name}_pt (1.0 / vec2(textureSize(${name}_raw, 0)))\n`;
      helpers += `vec4 ${name}_tex(vec2 uv) { return texture(${name}_raw, uv); }\n`;
    }
    const source = `#version 300 es
precision highp float;
precision highp int;
uniform vec2 uDstSize;
out vec4 outColor;
${helpers}
${body}
void main() { outColor = hook(); }`;
    passes.push({ binds, save, scale, source });
  }
  return passes;
}

function compile(gl, fragment) {
  const program = gl.createProgram();
  for (const [type, src] of [[gl.VERTEX_SHADER, VERTEX], [gl.FRAGMENT_SHADER, fragment]]) {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, src);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(shader) ?? "compile failed");
    gl.attachShader(program, shader);
  }
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program) ?? "link failed");
  return program;
}

/** The candidates by name: the passes each one draws, from the source to the output. */
export const CANDIDATES = {
  bilinear: [{ program: "bilinear", size: "dst" }],
  bicubic: [{ program: "bicubic", size: "dst" }],
  lanczos2: [{ program: "lanczos2", size: "dst" }],
  "sharpen-0.15+bicubic": [{ program: "sharpen", size: "src", k: 0.15 }, { program: "bicubic", size: "dst" }],
  "sharpen-0.30+bicubic": [{ program: "sharpen", size: "src", k: 0.3 }, { program: "bicubic", size: "dst" }],
  "bicubic+rcas-0.2": [{ program: "bicubic", size: "dst" }, { program: "rcas", size: "dst", sharp: 0.2 }],
  sgsr: [{ program: "sgsr", size: "dst" }],
  "fsr-easu": [{ program: "easu", size: "dst" }],
  "fsr-easu+rcas-0.2": [{ program: "easu", size: "dst" }, { program: "rcas", size: "dst", sharp: 0.2 }],
  "fsr-easu+rcas-0.6": [{ program: "easu", size: "dst" }, { program: "rcas", size: "dst", sharp: 0.6 }],
  "dering-8+fsr-easu": [{ program: "dering", size: "src", sigma: 8 / 255 }, { program: "easu", size: "dst" }],
  "dering-12+fsr-easu": [{ program: "dering", size: "src", sigma: 12 / 255 }, { program: "easu", size: "dst" }],
  "dering-16+fsr-easu": [{ program: "dering", size: "src", sigma: 16 / 255 }, { program: "easu", size: "dst" }],
  "dering-12+bicubic": [{ program: "dering", size: "src", sigma: 12 / 255 }, { program: "bicubic", size: "dst" }],
  "dering-12+sgsr": [{ program: "dering", size: "src", sigma: 12 / 255 }, { program: "sgsr", size: "dst" }],
  "dering-12+bilinear": [{ program: "dering", size: "src", sigma: 12 / 255 }, { program: "bilinear", size: "dst" }],
  "dering-16+sgsr": [{ program: "dering", size: "src", sigma: 16 / 255 }, { program: "sgsr", size: "dst" }],
  "dering-20+sgsr": [{ program: "dering", size: "src", sigma: 20 / 255 }, { program: "sgsr", size: "dst" }],
  "dering-12+sgsr-1.5": [{ program: "dering", size: "src", sigma: 12 / 255 }, { program: "sgsr", size: "dst", edge: 1.5 }],
  "dering-16+sgsr-1.5": [{ program: "dering", size: "src", sigma: 16 / 255 }, { program: "sgsr", size: "dst", edge: 1.5 }],
  "anime4k-s": [{ anime4k: "S" }],
  "anime4k-m": [{ anime4k: "M" }],
};

/** Runs candidates on one WebGL2 context: the source is uploaded once per frame, the output read back or shown. */
export class Bench {
  constructor(gl, anime4k = {}) {
    this.gl = gl;
    this.programs = {};
    this.targets = new Map();
    this.anime4k = {};
    this.floatTargets = Boolean(gl.getExtension("EXT_color_buffer_float") || gl.getExtension("EXT_color_buffer_half_float"));
    for (const [variant, text] of Object.entries(anime4k)) this.anime4k[variant] = translateAnime4K(text);
    this.src = this.texture(1, 1, gl.RGBA8, gl.RGBA, gl.UNSIGNED_BYTE, gl.LINEAR);
    this.srcSize = [1, 1];
    this.vao = gl.createVertexArray();
  }

  texture(w, h, internal, format, type, filter) {
    const gl = this.gl;
    const t = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, t);
    gl.texImage2D(gl.TEXTURE_2D, 0, internal, w, h, 0, format, type, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    return t;
  }

  /** A render target of the given size and kind ("u8" or "f16"), kept for reuse. */
  target(key, w, h, kind = "u8") {
    const id = `${key}:${w}x${h}:${kind}`;
    if (!this.targets.has(id)) {
      const gl = this.gl;
      const tex = kind === "f16"
        ? this.texture(w, h, gl.RGBA16F, gl.RGBA, gl.HALF_FLOAT, gl.NEAREST)
        : this.texture(w, h, gl.RGBA8, gl.RGBA, gl.UNSIGNED_BYTE, gl.LINEAR);
      const fb = gl.createFramebuffer();
      gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
      if (gl.checkFramebufferStatus(gl.FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE) throw new Error(`incomplete target ${id}`);
      this.targets.set(id, { tex, fb, w, h });
    }
    return this.targets.get(id);
  }

  program(name) {
    if (!this.programs[name]) {
      const sources = {
        bilinear: BILINEAR,
        bicubic: BICUBIC,
        lanczos2: LANCZOS2,
        downscale: DOWNSCALE,
        dering: DERING,
        sharpen: SHARPEN,
        sgsr: SGSR,
        easu: FSR_EASU,
        rcas: FSR_RCAS,
      };
      this.programs[name] = compile(this.gl, sources[name]);
    }
    return this.programs[name];
  }

  /** Uploads a frame: RGBA bytes (Uint8Array or Uint8ClampedArray) of w x h, row 0 at the top. */
  upload(rgba, w, h) {
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, this.src);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    if (this.srcSize[0] !== w || this.srcSize[1] !== h) {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(rgba.buffer, rgba.byteOffset, rgba.byteLength));
      this.srcSize = [w, h];
    } else {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(rgba.buffer, rgba.byteOffset, rgba.byteLength));
    }
  }

  drawPass(programName, input, inputSize, output, outputSize, uniforms = {}) {
    const gl = this.gl;
    const program = this.program(programName);
    gl.useProgram(program);
    gl.bindFramebuffer(gl.FRAMEBUFFER, output ? output.fb : null);
    gl.viewport(0, 0, outputSize[0], outputSize[1]);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, input);
    gl.uniform1i(gl.getUniformLocation(program, "uSrc"), 0);
    gl.uniform2f(gl.getUniformLocation(program, "uSrcSize"), inputSize[0], inputSize[1]);
    gl.uniform2f(gl.getUniformLocation(program, "uDstSize"), outputSize[0], outputSize[1]);
    if (uniforms.k !== undefined) gl.uniform1f(gl.getUniformLocation(program, "uK"), uniforms.k);
    if (uniforms.sharp !== undefined) gl.uniform1f(gl.getUniformLocation(program, "uSharp"), uniforms.sharp);
    if (uniforms.sigma !== undefined) gl.uniform1f(gl.getUniformLocation(program, "uSigma"), uniforms.sigma);
    if (programName === "sgsr") gl.uniform1f(gl.getUniformLocation(program, "uEdge"), uniforms.edge ?? 2.0);
    gl.bindVertexArray(this.vao);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  drawAnime4K(variant, dstW, dstH, toCanvas, slot) {
    const gl = this.gl;
    const passes = this.anime4k[variant];
    if (!passes || !this.floatTargets) throw new Error(`Anime4K ${variant} unavailable`);
    if (!passes.programs) passes.programs = passes.map((p) => compile(gl, p.source));
    const [sw, sh] = this.srcSize;
    const textures = { MAIN: this.src };
    let last = null;
    passes.forEach((pass, index) => {
      const w = sw * pass.scale, h = sh * pass.scale;
      const isLast = index === passes.length - 1;
      const out = this.target(isLast ? `a4k-${index}-s${slot}` : `a4k-${index}`, w, h, isLast ? "u8" : "f16");
      const program = passes.programs[index];
      gl.useProgram(program);
      gl.bindFramebuffer(gl.FRAMEBUFFER, out.fb);
      gl.viewport(0, 0, w, h);
      pass.binds.forEach((name, unit) => {
        gl.activeTexture(gl.TEXTURE0 + unit);
        gl.bindTexture(gl.TEXTURE_2D, textures[name]);
        gl.uniform1i(gl.getUniformLocation(program, `${name}_raw`), unit);
      });
      gl.uniform2f(gl.getUniformLocation(program, "uDstSize"), w, h);
      gl.bindVertexArray(this.vao);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      textures[pass.save] = out.tex;
      last = out;
    });
    // Twice the source; to a smaller size with a reducing Catmull-Rom, to a larger one with bicubic.
    const [lw, lh] = [sw * 2, sh * 2];
    if (dstW === lw && dstH === lh && !toCanvas) return last;
    const out = toCanvas ? null : this.target(`a4k-out-s${slot}`, dstW, dstH);
    this.drawPass(dstW < lw ? "downscale" : "bicubic", last.tex, [lw, lh], out, [dstW, dstH]);
    return out;
  }

  /**
   * Draws a candidate to an output of dstW x dstH (an offscreen target, or the canvas); returns the target. `slot`
   * picks one of several output targets: frames drawn in turn into different targets each end their render pass,
   * so that no GPU or driver drops a frame as overdrawn by the next one.
   */
  run(name, dstW, dstH, toCanvas = false, slot = 0) {
    const steps = CANDIDATES[name];
    if (steps[0].anime4k) return this.drawAnime4K(steps[0].anime4k, dstW, dstH, toCanvas, slot);
    let input = this.src, inputSize = this.srcSize, output = null;
    steps.forEach((step, index) => {
      const isLast = index === steps.length - 1;
      const size = step.size === "src" ? this.srcSize : [dstW, dstH];
      output = isLast && toCanvas ? null : this.target(isLast ? `${name}-${index}-s${slot}` : `${name}-${index}`, size[0], size[1]);
      this.drawPass(step.program, input, inputSize, output, size, step);
      if (output) { input = output.tex; inputSize = size; }
    });
    return output;
  }

  /** RGBA bytes of a target, row 0 at the top. */
  read(target) {
    const gl = this.gl;
    gl.bindFramebuffer(gl.FRAMEBUFFER, target.fb);
    const out = new Uint8Array(target.w * target.h * 4);
    gl.readPixels(0, 0, target.w, target.h, gl.RGBA, gl.UNSIGNED_BYTE, out);
    return out;
  }
}
