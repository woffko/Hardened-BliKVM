export const IMAGE_LIST_CHANGED_EVENT = 'hardened-blikvm:image-list-changed';

export function notifyImageListChanged() {
  window.dispatchEvent(new Event(IMAGE_LIST_CHANGED_EVENT));
}
