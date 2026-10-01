//! Safe wrapper over the narrow libjpeg-turbo (TurboJPEG) decompressor shim.
//!
//! The capture's JPEG frames decode straight into planar YCbCr at the
//! frame's own chroma subsampling, optionally at half size in the IDCT; the
//! caller turns those planes into I420.

use std::ffi::c_void;

/// Chroma subsampling of a frame, as TurboJPEG reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsampling {
    S444,
    S422,
    S420,
    Gray,
    /// 4:4:0, 4:1:1 and 4:4:1: valid JPEG, not produced by capture devices
    /// and not converted here.
    Other,
}

impl Subsampling {
    fn from_raw(value: i32) -> Self {
        // TJSAMP_444, TJSAMP_422, TJSAMP_420 and TJSAMP_GRAY in turbojpeg.h.
        match value {
            0 => Self::S444,
            1 => Self::S422,
            2 => Self::S420,
            3 => Self::Gray,
            _ => Self::Other,
        }
    }

    fn raw(self) -> i32 {
        match self {
            Self::S444 => 0,
            Self::S422 => 1,
            Self::S420 => 2,
            Self::Gray => 3,
            Self::Other => -1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub width: usize,
    pub height: usize,
    pub subsampling: Subsampling,
}

unsafe extern "C" {
    fn bk_jpeg_open() -> *mut c_void;
    fn bk_jpeg_close(handle: *mut c_void);
    fn bk_jpeg_set_fast_dct(handle: *mut c_void, fast: i32) -> i32;
    fn bk_jpeg_error(handle: *mut c_void) -> *const std::ffi::c_char;
    fn bk_jpeg_header(
        handle: *mut c_void,
        jpeg: *const u8,
        size: usize,
        width: *mut i32,
        height: *mut i32,
        subsamp: *mut i32,
    ) -> i32;
    fn bk_jpeg_plane(
        component: i32,
        width: i32,
        height: i32,
        subsamp: i32,
        plane_width: *mut i32,
        plane_height: *mut i32,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn bk_jpeg_decode(
        handle: *mut c_void,
        jpeg: *const u8,
        size: usize,
        denom: i32,
        y: *mut u8,
        y_stride: i32,
        u: *mut u8,
        u_stride: i32,
        v: *mut u8,
        v_stride: i32,
    ) -> i32;
}

/// One plane's size at a decoded size: (width, height) in samples.
pub fn plane_size(
    component: usize,
    width: usize,
    height: usize,
    subsampling: Subsampling,
) -> Option<(usize, usize)> {
    let (mut plane_width, mut plane_height) = (0_i32, 0_i32);
    // SAFETY: pure arithmetic on plain integers; the out pointers are locals.
    let status = unsafe {
        bk_jpeg_plane(
            i32::try_from(component).ok()?,
            i32::try_from(width).ok()?,
            i32::try_from(height).ok()?,
            subsampling.raw(),
            &mut plane_width,
            &mut plane_height,
        )
    };
    (status == 0).then_some((
        usize::try_from(plane_width).ok()?,
        usize::try_from(plane_height).ok()?,
    ))
}

/// Destination planes for one decode; each must hold its plane's
/// `stride * height` bytes at the decoded size.
pub struct Planes<'a> {
    pub y: &'a mut [u8],
    pub y_stride: usize,
    pub u: &'a mut [u8],
    pub u_stride: usize,
    pub v: &'a mut [u8],
    pub v_stride: usize,
}

pub struct TurboDecoder {
    handle: *mut c_void,
}

// One decoder is used from one thread at a time; TurboJPEG keeps its state in
// the handle.
unsafe impl Send for TurboDecoder {}

impl TurboDecoder {
    pub fn new(fast_dct: bool) -> Option<Self> {
        // SAFETY: bk_jpeg_open takes no arguments and returns an owned handle
        // or NULL.
        let handle = unsafe { bk_jpeg_open() };
        if handle.is_null() {
            return None;
        }
        let decoder = Self { handle };
        // SAFETY: the handle is live until drop.
        if unsafe { bk_jpeg_set_fast_dct(decoder.handle, i32::from(fast_dct)) } != 0 {
            return None;
        }
        Some(decoder)
    }

    /// Chooses the IDCT for the next decodes: the fast one is less exact.
    pub fn set_fast_dct(&mut self, fast: bool) -> bool {
        // SAFETY: the handle is live until drop.
        unsafe { bk_jpeg_set_fast_dct(self.handle, i32::from(fast)) == 0 }
    }

    /// The last error TurboJPEG reported, for logs.
    pub fn last_error(&self) -> String {
        // SAFETY: the handle is live; TurboJPEG returns a NUL-terminated
        // string it owns, copied here before any other call.
        let text = unsafe { bk_jpeg_error(self.handle) };
        if text.is_null() {
            return String::new();
        }
        // SAFETY: non-null and NUL-terminated, as above.
        unsafe { std::ffi::CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn header(&mut self, jpeg: &[u8]) -> Option<Header> {
        let (mut width, mut height, mut subsamp) = (0_i32, 0_i32, 0_i32);
        // SAFETY: the handle is live, jpeg is a readable slice of its length
        // and the out pointers are locals.
        let status = unsafe {
            bk_jpeg_header(
                self.handle,
                jpeg.as_ptr(),
                jpeg.len(),
                &mut width,
                &mut height,
                &mut subsamp,
            )
        };
        (status == 0).then_some(Header {
            width: usize::try_from(width).ok()?,
            height: usize::try_from(height).ok()?,
            subsampling: Subsampling::from_raw(subsamp),
        })
    }

    /// Decodes `jpeg` (whose header gave `header`) at 1/`denom` of its size
    /// into `planes`. Returns false if the frame is damaged or the planes are
    /// too small for it.
    pub fn decode(
        &mut self,
        jpeg: &[u8],
        header: Header,
        denom: usize,
        planes: Planes<'_>,
    ) -> bool {
        if !(denom == 1 || denom == 2) {
            return false;
        }
        let (width, height) = (header.width.div_ceil(denom), header.height.div_ceil(denom));
        let gray = header.subsampling == Subsampling::Gray;
        let fits = |component: usize, plane: &[u8], stride: usize| {
            plane_size(component, width, height, header.subsampling).is_some_and(
                |(plane_width, plane_height)| {
                    stride >= plane_width
                        && stride
                            .checked_mul(plane_height)
                            .is_some_and(|need| plane.len() >= need)
                },
            )
        };
        if header.subsampling == Subsampling::Other
            || !fits(0, planes.y, planes.y_stride)
            || (!gray
                && (!fits(1, planes.u, planes.u_stride) || !fits(2, planes.v, planes.v_stride)))
        {
            return false;
        }
        let stride = |value: usize| i32::try_from(value).ok();
        let (Some(y_stride), Some(u_stride), Some(v_stride)) = (
            stride(planes.y_stride),
            stride(planes.u_stride),
            stride(planes.v_stride),
        ) else {
            return false;
        };
        let (u, v) = if gray {
            (std::ptr::null_mut(), std::ptr::null_mut())
        } else {
            (planes.u.as_mut_ptr(), planes.v.as_mut_ptr())
        };
        // SAFETY: the handle is live; jpeg is readable for its length; every
        // plane was checked above to hold stride * plane height bytes at the
        // decoded size, which is what TurboJPEG writes.
        let status = unsafe {
            bk_jpeg_decode(
                self.handle,
                jpeg.as_ptr(),
                jpeg.len(),
                denom as i32,
                planes.y.as_mut_ptr(),
                y_stride,
                u,
                u_stride,
                v,
                v_stride,
            )
        };
        status == 0
    }
}

impl Drop for TurboDecoder {
    fn drop(&mut self) {
        // SAFETY: the handle came from bk_jpeg_open and is closed once.
        unsafe { bk_jpeg_close(self.handle) }
    }
}
