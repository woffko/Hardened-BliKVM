// Narrow C boundary around libvpx's VP8 realtime encoder. Rust never sees
// libvpx structures; every call validates its arguments and returns a code.
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "vpx/vp8cx.h"
#include "vpx/vpx_encoder.h"

struct bk_vp8 {
  vpx_codec_ctx_t codec;
  vpx_image_t image;
  unsigned width;
  unsigned height;
};

int bk_vp8_open(unsigned width, unsigned height, unsigned fps, unsigned kbps,
                unsigned threads, unsigned keyframe_interval,
                struct bk_vp8 **out) {
  vpx_codec_enc_cfg_t cfg;
  struct bk_vp8 *enc;
  if (out == NULL || width < 16 || height < 16 || width > 4096 ||
      height > 4096 || (width & 1) || (height & 1) || fps == 0 || fps > 60 ||
      kbps < 64 || kbps > 20000 || threads == 0 || threads > 8 ||
      keyframe_interval == 0 || keyframe_interval > 600)
    return -1;
  if (vpx_codec_enc_config_default(vpx_codec_vp8_cx(), &cfg, 0) != VPX_CODEC_OK)
    return -2;
  cfg.g_w = width;
  cfg.g_h = height;
  cfg.g_timebase.num = 1;
  cfg.g_timebase.den = 1000000;
  cfg.g_threads = threads;
  cfg.g_lag_in_frames = 0;
  cfg.g_error_resilient = VPX_ERROR_RESILIENT_DEFAULT;
  cfg.g_pass = VPX_RC_ONE_PASS;
  cfg.rc_end_usage = VPX_CBR;
  cfg.rc_target_bitrate = kbps;
  cfg.rc_dropframe_thresh = 0;
  cfg.rc_min_quantizer = 4;
  cfg.rc_max_quantizer = 56;
  cfg.rc_buf_sz = 1000;
  cfg.rc_buf_initial_sz = 500;
  cfg.rc_buf_optimal_sz = 600;
  cfg.kf_mode = VPX_KF_AUTO;
  cfg.kf_min_dist = 0;
  cfg.kf_max_dist = keyframe_interval;
  enc = calloc(1, sizeof(*enc));
  if (enc == NULL) return -3;
  if (vpx_codec_enc_init(&enc->codec, vpx_codec_vp8_cx(), &cfg, 0) != VPX_CODEC_OK) {
    free(enc);
    return -4;
  }
  // Fastest realtime preset; screen content tuning suits desktops.
  if (vpx_codec_control(&enc->codec, VP8E_SET_CPUUSED, 16) != VPX_CODEC_OK ||
      vpx_codec_control(&enc->codec, VP8E_SET_SCREEN_CONTENT_MODE, 1) != VPX_CODEC_OK ||
      vpx_codec_control(&enc->codec, VP8E_SET_STATIC_THRESHOLD, 1) != VPX_CODEC_OK ||
      vpx_codec_control(&enc->codec, VP8E_SET_TOKEN_PARTITIONS, 0) != VPX_CODEC_OK) {
    vpx_codec_destroy(&enc->codec);
    free(enc);
    return -5;
  }
  enc->width = width;
  enc->height = height;
  *out = enc;
  return 0;
}

// Encode one I420 frame. `emit` receives each compressed packet. The planes
// are borrowed for the duration of the call only.
int bk_vp8_encode(struct bk_vp8 *enc, const uint8_t *y, const uint8_t *u,
                  const uint8_t *v, unsigned y_stride, unsigned uv_stride,
                  int64_t pts_us, int64_t duration_us, int force_key,
                  void (*emit)(void *context, const uint8_t *data, size_t size,
                               int key_frame),
                  void *context) {
  const vpx_codec_cx_pkt_t *packet;
  vpx_codec_iter_t iter = NULL;
  if (enc == NULL || y == NULL || u == NULL || v == NULL || emit == NULL ||
      y_stride < enc->width || uv_stride < (enc->width + 1) / 2 || pts_us < 0 ||
      duration_us <= 0)
    return -1;
  if (vpx_img_wrap(&enc->image, VPX_IMG_FMT_I420, enc->width, enc->height, 1,
                   (unsigned char *)y) == NULL)
    return -2;
  enc->image.planes[VPX_PLANE_Y] = (unsigned char *)y;
  enc->image.planes[VPX_PLANE_U] = (unsigned char *)u;
  enc->image.planes[VPX_PLANE_V] = (unsigned char *)v;
  enc->image.stride[VPX_PLANE_Y] = (int)y_stride;
  enc->image.stride[VPX_PLANE_U] = (int)uv_stride;
  enc->image.stride[VPX_PLANE_V] = (int)uv_stride;
  if (vpx_codec_encode(&enc->codec, &enc->image, pts_us, (unsigned long)duration_us,
                       force_key ? VPX_EFLAG_FORCE_KF : 0,
                       VPX_DL_REALTIME) != VPX_CODEC_OK)
    return -3;
  while ((packet = vpx_codec_get_cx_data(&enc->codec, &iter)) != NULL) {
    if (packet->kind == VPX_CODEC_CX_FRAME_PKT)
      emit(context, packet->data.frame.buf, packet->data.frame.sz,
           (packet->data.frame.flags & VPX_FRAME_IS_KEY) != 0);
  }
  return 0;
}

int bk_vp8_set_bitrate(struct bk_vp8 *enc, unsigned kbps) {
  vpx_codec_enc_cfg_t cfg;
  if (enc == NULL || kbps < 64 || kbps > 20000) return -1;
  cfg = *enc->codec.config.enc;
  cfg.rc_target_bitrate = kbps;
  return vpx_codec_enc_config_set(&enc->codec, &cfg) == VPX_CODEC_OK ? 0 : -2;
}

void bk_vp8_close(struct bk_vp8 *enc) {
  if (enc == NULL) return;
  vpx_codec_destroy(&enc->codec);
  free(enc);
}
