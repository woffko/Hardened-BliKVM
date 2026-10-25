/**
 * Where the pointer falls in the target's picture. The screen element is
 * laid out at the picture's shape, and the picture drawn into it; the views
 * state the picture's own size on the element (`data-picture-width` and
 * `data-picture-height`), because what the element holds need not have it:
 * an upscaled canvas has the drawn size, capped (docs/design/screen-upscaling-v1.md).
 */

export type Size = { width: number; height: number };

type PictureElement = {
  dataset?: DOMStringMap;
  tagName?: string;
  videoWidth?: number;
  videoHeight?: number;
  naturalWidth?: number;
  naturalHeight?: number;
  width?: number;
  height?: number;
};

const positive = (width: number | undefined, height: number | undefined): Size | null =>
  width !== undefined && height !== undefined && width > 0 && height > 0 ? { width, height } : null;

/** The attributes a view puts on its screen element for the picture's size. */
export function pictureAttributes(
  size: Size | null | undefined
): Record<string, number | undefined> {
  return {
    'data-picture-width': size?.width && size.width > 0 ? size.width : undefined,
    'data-picture-height': size?.height && size.height > 0 ? size.height : undefined
  };
}

/**
 * The picture's size: as the view states it, else the video's, the image's
 * or the canvas's own; null when none is known.
 */
export function pictureSize(element: PictureElement): Size | null {
  const stated = positive(
    Number(element.dataset?.pictureWidth ?? 0),
    Number(element.dataset?.pictureHeight ?? 0)
  );
  if (stated) return stated;
  const video = positive(element.videoWidth, element.videoHeight);
  if (video) return video;
  const image = positive(element.naturalWidth, element.naturalHeight);
  if (image) return image;
  if (element.tagName === 'CANVAS') return positive(element.width, element.height);
  return null;
}

/**
 * Where a point of the page falls in a picture of `picture`'s shape drawn
 * as large as it fits in `rect`, centred: 0 to 1 each way inside the
 * picture, beyond outside it. Without a picture, the element's own extent.
 */
export function pointInPicture(
  clientX: number,
  clientY: number,
  rect: { left: number; top: number; width: number; height: number },
  picture: Size | null
): { x: number; y: number } {
  if (!picture) {
    return { x: (clientX - rect.left) / rect.width, y: (clientY - rect.top) / rect.height };
  }
  const pictureRatio = picture.width / picture.height;
  const elementRatio = rect.width / rect.height;
  let width = rect.width;
  let height = rect.height;
  let offsetX = 0;
  let offsetY = 0;
  if (pictureRatio > elementRatio) {
    height = rect.width / pictureRatio;
    offsetY = (rect.height - height) / 2;
  } else {
    width = rect.height * pictureRatio;
    offsetX = (rect.width - width) / 2;
  }
  return {
    x: (clientX - rect.left - offsetX) / width,
    y: (clientY - rect.top - offsetY) / height
  };
}
