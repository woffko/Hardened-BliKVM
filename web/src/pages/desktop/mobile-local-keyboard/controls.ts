export const MOBILE_LOCAL_KEYBOARD_INPUT_ID = 'hardened-blikvm-mobile-local-keyboard';

export function focusMobileLocalKeyboard() {
  const input = document.getElementById(
    MOBILE_LOCAL_KEYBOARD_INPUT_ID
  ) as HTMLTextAreaElement | null;
  input?.focus({ preventScroll: true });
}

export function blurMobileLocalKeyboard() {
  const input = document.getElementById(
    MOBILE_LOCAL_KEYBOARD_INPUT_ID
  ) as HTMLTextAreaElement | null;
  input?.blur();
}
