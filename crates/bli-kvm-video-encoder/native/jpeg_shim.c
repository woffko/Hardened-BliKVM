// Narrow C boundary around libjpeg-turbo's TurboJPEG decompressor. Rust never
// sees TurboJPEG structures; every call validates its arguments and returns a
// code. TurboJPEG reports errors through return values (it never calls exit),
// and a warning stops the decode, so a damaged capture frame is refused
// instead of decoded with grey blocks.
#include <stddef.h>
#include <stdint.h>

#include "turbojpeg.h"

// Bounds for a capture frame: no frame larger than 4096x4096, progressive
// frames limited in scans and buffer memory (the capture sends baseline).
#define BK_JPEG_MAX_DIMENSION 4096
#define BK_JPEG_MAX_SCANS 64
#define BK_JPEG_MAX_MEMORY_MB 64

void *bk_jpeg_open(void) {
  tjhandle handle = tj3Init(TJINIT_DECOMPRESS);
  if (handle == NULL) return NULL;
  if (tj3Set(handle, TJPARAM_STOPONWARNING, 1) != 0 ||
      tj3Set(handle, TJPARAM_MAXPIXELS,
             BK_JPEG_MAX_DIMENSION * BK_JPEG_MAX_DIMENSION) != 0 ||
      tj3Set(handle, TJPARAM_SCANLIMIT, BK_JPEG_MAX_SCANS) != 0 ||
      tj3Set(handle, TJPARAM_MAXMEMORY, BK_JPEG_MAX_MEMORY_MB) != 0) {
    tj3Destroy(handle);
    return NULL;
  }
  return handle;
}

void bk_jpeg_close(void *handle) {
  if (handle != NULL) tj3Destroy(handle);
}

// The last error TurboJPEG reported on this handle, for logs.
const char *bk_jpeg_error(void *handle) {
  return handle != NULL ? tj3GetErrorStr(handle) : "no handle";
}

// The accurate integer IDCT is the default; the fast one is less exact.
int bk_jpeg_set_fast_dct(void *handle, int fast) {
  if (handle == NULL) return -1;
  return tj3Set(handle, TJPARAM_FASTDCT, fast ? 1 : 0) == 0 ? 0 : -2;
}

// Reads the frame header. Only 8-bit lossy YCbCr or grayscale frames within
// the bounds are accepted. subsamp is a TJSAMP value.
int bk_jpeg_header(void *handle, const uint8_t *jpeg, size_t size, int *width,
                   int *height, int *subsamp) {
  int colorspace;
  if (handle == NULL || jpeg == NULL || size == 0 || width == NULL ||
      height == NULL || subsamp == NULL)
    return -1;
  if (tj3DecompressHeader(handle, jpeg, size) != 0) return -2;
  *width = tj3Get(handle, TJPARAM_JPEGWIDTH);
  *height = tj3Get(handle, TJPARAM_JPEGHEIGHT);
  *subsamp = tj3Get(handle, TJPARAM_SUBSAMP);
  colorspace = tj3Get(handle, TJPARAM_COLORSPACE);
  if (*width < 1 || *height < 1 || *width > BK_JPEG_MAX_DIMENSION ||
      *height > BK_JPEG_MAX_DIMENSION)
    return -3;
  if (tj3Get(handle, TJPARAM_LOSSLESS) != 0 ||
      tj3Get(handle, TJPARAM_PRECISION) != 8)
    return -4;
  if (colorspace != TJCS_YCbCr && colorspace != TJCS_GRAY) return -4;
  return 0;
}

// Size of one plane of the frame at the given (scaled) size, as TurboJPEG
// lays it out: plane width and height in samples.
int bk_jpeg_plane(int component, int width, int height, int subsamp,
                  int *plane_width, int *plane_height) {
  if (plane_width == NULL || plane_height == NULL) return -1;
  *plane_width = tj3YUVPlaneWidth(component, width, subsamp);
  *plane_height = tj3YUVPlaneHeight(component, height, subsamp);
  return (*plane_width > 0 && *plane_height > 0) ? 0 : -2;
}

// Decodes the frame whose header was just read, at 1/denom of its size
// (denom 1 or 2, in the IDCT), into separate Y, Cb and Cr planes without
// colour conversion. A grayscale frame fills only the Y plane. The caller
// sized each plane with bk_jpeg_plane and passes its row stride.
int bk_jpeg_decode(void *handle, const uint8_t *jpeg, size_t size, int denom,
                   uint8_t *y, int y_stride, uint8_t *u, int u_stride,
                   uint8_t *v, int v_stride) {
  tjscalingfactor scaling;
  unsigned char *planes[3];
  int strides[3];
  if (handle == NULL || jpeg == NULL || size == 0 || y == NULL ||
      (denom != 1 && denom != 2) || y_stride < 1)
    return -1;
  scaling.num = 1;
  scaling.denom = denom;
  if (tj3SetScalingFactor(handle, scaling) != 0) return -2;
  planes[0] = y;
  planes[1] = u;
  planes[2] = v;
  strides[0] = y_stride;
  strides[1] = u_stride;
  strides[2] = v_stride;
  return tj3DecompressToYUVPlanes8(handle, jpeg, size, planes, strides) == 0
             ? 0
             : -3;
}
