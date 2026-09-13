import { useCallback, useRef } from 'react';
import { Modal } from 'antd';
import { useAtom } from 'jotai';

import { focusConfirmedAtom } from '@/jotai/restricted-control.ts';

const CONFIRM_TITLE = 'Confirm remote control';
const CONFIRM_BODY =
  'Continue only if the KVM video shows the target you intend to control. ' +
  'This binds the current session to the visible target for a short time.';

/**
 * Requires an explicit operator confirmation before any restricted HID or
 * power action. The confirmation is requested at most once at a time.
 */
export function useFocusConfirmation() {
  const [confirmed, setConfirmed] = useAtom(focusConfirmedAtom);
  const pendingRef = useRef<Promise<boolean> | null>(null);

  const ensureConfirmed = useCallback(async () => {
    if (confirmed) {
      return true;
    }
    if (pendingRef.current) {
      return pendingRef.current;
    }
    const pending = new Promise<boolean>((resolve) => {
      Modal.confirm({
        title: CONFIRM_TITLE,
        content: CONFIRM_BODY,
        okText: 'Confirm and control',
        cancelText: 'Cancel',
        onOk: () => {
          setConfirmed(true);
          resolve(true);
        },
        onCancel: () => resolve(false)
      });
    }).finally(() => {
      pendingRef.current = null;
    });
    pendingRef.current = pending;
    return pending;
  }, [confirmed, setConfirmed]);

  return { confirmed, ensureConfirmed };
}
