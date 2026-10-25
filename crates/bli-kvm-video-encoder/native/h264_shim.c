// Narrow C boundary around x264's realtime H.264 encoder. Rust never sees
// x264 structures; every call validates its arguments and returns a code.
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "x264.h"

struct bk_h264 {
  x264_t *encoder;
  x264_picture_t picture;
  unsigned width;
  unsigned height;
};

// preset: 0 = superfast, 1 = ultrafast. Both with tune zerolatency and the
// constrained baseline profile browsers decode over WebRTC.
int bk_h264_open(unsigned width, unsigned height, unsigned fps, unsigned kbps,
                 unsigned threads, unsigned keyframe_interval, unsigned preset,
                 struct bk_h264 **out) {
  x264_param_t param;
  struct bk_h264 *enc;
  if (out == NULL || width < 16 || height < 16 || width > 4096 ||
      height > 4096 || (width & 1) || (height & 1) || fps == 0 || fps > 60 ||
      kbps < 64 || kbps > 20000 || threads == 0 || threads > 8 ||
      keyframe_interval == 0 || keyframe_interval > 600 || preset > 1)
    return -1;
  if (x264_param_default_preset(&param, preset ? "ultrafast" : "superfast",
                                "zerolatency") < 0)
    return -2;
  param.i_log_level = X264_LOG_NONE;
  param.i_width = (int)width;
  param.i_height = (int)height;
  param.i_csp = X264_CSP_I420;
  param.i_fps_num = fps;
  param.i_fps_den = 1;
  // Constant frame rate, as zerolatency sets it: with variable-rate input
  // x264 holds each frame until the next one arrives, to learn its duration,
  // and frames come only when the picture changes. Timestamps in microseconds
  // still travel with the frames.
  param.b_vfr_input = 0;
  param.i_timebase_num = 1;
  param.i_timebase_den = 1000000;
  param.i_threads = (int)threads;
  param.i_keyint_max = (int)keyframe_interval;
  param.i_keyint_min = 1;
  param.b_intra_refresh = 0;
  // SPS and PPS before every key frame, so a joining viewer can start.
  param.b_repeat_headers = 1;
  param.b_annexb = 1;
  param.rc.i_rc_method = X264_RC_ABR;
  param.rc.i_bitrate = (int)kbps;
  param.rc.i_vbv_max_bitrate = (int)kbps;
  param.rc.i_vbv_buffer_size = (int)kbps;
  // What the samples mean: the capture's JPEG YCbCr of the full range with
  // the BT.601 matrix, of an sRGB picture, given to x264 as they are. The SPS
  // says so as the video engine's does (bli-kvm-ve-encoder, headers.rs): a
  // stream that says nothing is stretched by the browser as the narrow range.
  param.vui.b_fullrange = 1;
  param.vui.i_colorprim = 1;   // BT.709, which sRGB shares
  param.vui.i_transfer = 13;   // IEC 61966-2-1, sRGB
  param.vui.i_colmatrix = 6;   // SMPTE 170M, the BT.601 matrix
  if (x264_param_apply_profile(&param, "baseline") < 0) return -2;
  enc = calloc(1, sizeof(*enc));
  if (enc == NULL) return -3;
  enc->encoder = x264_encoder_open(&param);
  if (enc->encoder == NULL) {
    free(enc);
    return -4;
  }
  x264_picture_init(&enc->picture);
  enc->picture.img.i_csp = X264_CSP_I420;
  enc->picture.img.i_plane = 3;
  enc->width = width;
  enc->height = height;
  *out = enc;
  return 0;
}

// Encode one I420 frame. `emit` receives the access unit (Annex B, all NAL
// units back to back). The planes are borrowed for the duration of the call.
int bk_h264_encode(struct bk_h264 *enc, const uint8_t *y, const uint8_t *u,
                   const uint8_t *v, unsigned y_stride, unsigned uv_stride,
                   int64_t pts_us, int force_key,
                   void (*emit)(void *context, const uint8_t *data, size_t size,
                                int key_frame),
                   void *context) {
  x264_nal_t *nals = NULL;
  int count = 0;
  int size;
  x264_picture_t output;
  if (enc == NULL || y == NULL || u == NULL || v == NULL || emit == NULL ||
      y_stride < enc->width || uv_stride < (enc->width + 1) / 2 || pts_us < 0)
    return -1;
  enc->picture.img.plane[0] = (uint8_t *)y;
  enc->picture.img.plane[1] = (uint8_t *)u;
  enc->picture.img.plane[2] = (uint8_t *)v;
  enc->picture.img.i_stride[0] = (int)y_stride;
  enc->picture.img.i_stride[1] = (int)uv_stride;
  enc->picture.img.i_stride[2] = (int)uv_stride;
  enc->picture.i_pts = pts_us;
  enc->picture.i_type = force_key ? X264_TYPE_IDR : X264_TYPE_AUTO;
  size = x264_encoder_encode(enc->encoder, &nals, &count, &enc->picture, &output);
  if (size < 0) return -3;
  // x264 keeps the payloads of one call's NAL units contiguous.
  if (size > 0 && count > 0)
    emit(context, nals[0].p_payload, (size_t)size, output.b_keyframe != 0);
  return 0;
}

// Changes the target bitrate of a running encoder: x264 applies it from the
// next frame (rate control and the VBV bound move together).
int bk_h264_set_bitrate(struct bk_h264 *enc, unsigned kbps) {
  x264_param_t param;
  if (enc == NULL || kbps < 64 || kbps > 20000) return -1;
  x264_encoder_parameters(enc->encoder, &param);
  param.rc.i_bitrate = (int)kbps;
  param.rc.i_vbv_max_bitrate = (int)kbps;
  param.rc.i_vbv_buffer_size = (int)kbps;
  return x264_encoder_reconfig(enc->encoder, &param) == 0 ? 0 : -2;
}

void bk_h264_close(struct bk_h264 *enc) {
  if (enc == NULL) return;
  x264_encoder_close(enc->encoder);
  free(enc);
}
