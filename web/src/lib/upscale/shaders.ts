/**
 * The upscaler's programs (docs/design/screen-upscaling-v1.md): a deringing
 * pass over the captured frame, then Qualcomm's SGSR 1 to the drawn size, and
 * a plain bilinear pass for when the picture is not enlarged. Chosen at gate 0
 * of the plan (docs/baselines/audit-20261008-upscaling-gate0.md).
 *
 * SGSR 1 is ported from sgsr1_shader_mobile.frag of
 * https://github.com/SnapdragonStudios/snapdragon-gsr (commit d926f074bcb9):
 * Copyright (c) 2023-2025, Qualcomm Innovation Center, Inc. All rights
 * reserved. SPDX-License-Identifier: BSD-3-Clause. The licence's text ships
 * beside the page as THIRD-PARTY-NOTICES.txt. Changes: textureGather and the
 * bilinear sample replaced by texelFetch, the edge sharpness a uniform, a
 * part of the frame drawn to the whole output, the output's rows from the
 * top.
 *
 * Conventions: the frame's row 0 is its top row in the source texture and in
 * the deringed one (texture coordinates grow downward); the canvas's rows are
 * counted from the bottom, so the last pass turns gl_FragCoord upside down.
 */

/** A triangle that covers the output. */
export const VERTEX = `#version 300 es
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;

/**
 * The deringing pass, at the frame's size: a 5x5 bilateral filter whose range
 * weight comes from the luma difference (sigma uSigma), which smooths the
 * capture's JPEG ringing beside text (a few levels) and keeps the text's own
 * edges (a hundred levels and more). For the board's VP8 picture it first
 * undoes the browser's narrow-range stretch with the table of vp8-range.ts;
 * with sigma 0 it does only that.
 */
export const DERING = `#version 300 es
precision highp float;
precision highp int;
uniform sampler2D uSource;
uniform float uSigma;
uniform bool uVp8Range;
out vec4 outColor;
const vec3 TO_LUMA = vec3(0.299, 0.587, 0.114);
vec3 level(ivec2 p) {
  vec3 c = texelFetch(uSource, clamp(p, ivec2(0), textureSize(uSource, 0) - 1), 0).rgb;
  if (!uVp8Range) return c;
  // drawn / 255 to (drawn * 219 / 255 + 16) / 255; black and white stay.
  vec3 ends = vec3(lessThan(c, vec3(0.5 / 255.0))) + vec3(greaterThan(c, vec3(254.5 / 255.0)));
  return mix((c * 219.0 + 16.0) / 255.0, c, ends);
}
void main() {
  ivec2 p = ivec2(gl_FragCoord.xy);
  vec3 centre = level(p);
  if (uSigma <= 0.0) {
    // Only VP8's table: the plain picture.
    outColor = vec4(centre, 1.0);
    return;
  }
  float centreLuma = dot(centre, TO_LUMA);
  float rangeScale = 1.0 / (2.0 * uSigma * uSigma);
  vec3 sum = vec3(0.0);
  float weights = 0.0;
  for (int j = -2; j <= 2; j++) {
    for (int i = -2; i <= 2; i++) {
      vec3 c = level(p + ivec2(i, j));
      float d = dot(c, TO_LUMA) - centreLuma;
      float w = exp(-float(i * i + j * j) * (1.0 / 4.5) - d * d * rangeScale);
      sum += c * w;
      weights += w;
    }
  }
  outColor = vec4(sum / weights, 1.0);
}`;

const OUTPUT_HEADER = `#version 300 es
precision highp float;
precision highp int;
uniform sampler2D uFrame;
uniform vec2 uFrameSize;
uniform vec4 uCrop;
uniform vec2 uOutSize;
out vec4 outColor;
// The output pixel's centre, rows from the top, and where it falls in the
// frame, in pixels (pixel centres at k + 0.5).
vec2 outPos() { return vec2(gl_FragCoord.x, uOutSize.y - gl_FragCoord.y); }
vec2 framePos() { return uCrop.xy + outPos() / uOutSize * uCrop.zw; }
`;

/** The plain pass: the frame's part drawn by the GPU's bilinear filter. */
export const BILINEAR = `${OUTPUT_HEADER}
void main() { outColor = vec4(texture(uFrame, framePos() / uFrameSize).rgb, 1.0); }`;

/**
 * SGSR 1, OperationMode 1 (RGBA, green as the luma). The edge test is a
 * threshold, and the same frame should take the same branch on every GPU:
 * the bilinear sample is computed from the four nearest texels, which the
 * test reads anyway, rather than by the GPU's filter, whose precision
 * differs; and the threshold is 8.37 levels, not upstream's 8. With the
 * sample between its two neighbours the vote is twice their difference, so a
 * difference of 4 levels made a tie at 8 that each GPU rounded its own way.
 * A frame enlarged by 4/3 or 2 has votes in 4096ths of a level, which never
 * come within 1e-4 of a level of 8.37.
 */
export const SGSR = `${OUTPUT_HEADER}
uniform float uEdgeSharpness;
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
vec3 texel(ivec2 p) { return texelFetch(uFrame, clamp(p, ivec2(0), ivec2(uFrameSize) - 1), 0).rgb; }
float green(ivec2 p) { return texel(p).g; }
void main() {
  float edgeThreshold = 8.37 / 255.0;
  vec2 pos = framePos();
  // The texel to the upper left of the sample (i, j) and the fractions to the
  // next ones: upstream's imgCoordPixel is (i, j + 1), its pl is f.
  vec2 at = pos - 0.5;
  ivec2 ij = ivec2(floor(at));
  vec2 pl = at - floor(at);
  vec3 c00 = texel(ij), c10 = texel(ij + ivec2(1, 0)), c01 = texel(ij + ivec2(0, 1)), c11 = texel(ij + ivec2(1, 1));
  vec4 color = vec4(mix(mix(c00, c10, pl.x), mix(c01, c11, pl.x), pl.y), 0.0);
  // textureGather's four at the corner (i, j + 1): x = (i - 1, j + 1), y = (i, j + 1), z = (i, j), w = (i - 1, j).
  vec4 left = vec4(0.0, c01.g, c00.g, 0.0);
  float edgeVote = abs(left.z - left.y) + abs(color.g - left.y) + abs(color.g - left.z);
  if (edgeVote > edgeThreshold) {
    left.x = green(ij + ivec2(-1, 1));
    left.w = green(ij + ivec2(-1, 0));
    // At the corner (i + 2, j + 1): (i + 1, j + 1), (i + 2, j + 1), (i + 2, j), (i + 1, j).
    vec4 right = vec4(c11.g, green(ij + ivec2(2, 1)), green(ij + ivec2(2, 0)), c10.g);
    // Above: (i, j - 1), (i + 1, j - 1); below: (i + 1, j + 2), (i, j + 2).
    vec4 upDown = vec4(green(ij + ivec2(0, -1)), green(ij + ivec2(1, -1)), green(ij + ivec2(1, 2)), green(ij + ivec2(0, 2)));
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
    finalY = clamp(uEdgeSharpness * finalY, minY, maxY);
    float deltaY = clamp(finalY - color.w, -23.0 / 255.0, 23.0 / 255.0);
    color.rgb = clamp(color.rgb + deltaY, 0.0, 1.0);
  }
  outColor = vec4(color.rgb, 1.0);
}`;
