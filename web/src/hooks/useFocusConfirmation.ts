import { useCallback, useEffect } from 'react';
import { Modal } from 'antd';
import { useAtom } from 'jotai';

import { focusConfirmedAtom } from '@/jotai/restricted-control.ts';

const CONFIRM_TITLE = 'Confirm remote control';
const CONFIRM_BODY =
  'Continue only if the KVM video shows the target you intend to control. ' +
  'This binds the current session to the visible target for a short time.';

/**
 * Shared confirmation state so that every queued HID report and every mounted
 * control surface observes the same decision. The module-level flag and the
 * single pending promise prevent a burst of queued events from opening a new
 * modal after the operator already confirmed.
 */
let confirmedGlobal = false;
let pendingConfirmation: Promise<boolean> | null = null;

/**
 * Requires an explicit operator confirmation before any restricted HID or
 * power action. The confirmation is requested at most once at a time and is
 * never re-requested while it is being decided.
 */
export function useFocusConfirmation() {
  const [confirmed, setConfirmed] = useAtom(focusConfirmedAtom);

  useEffect(() => {
    if (confirmed) {
      confirmedGlobal = true;
    }
  }, [confirmed]);

  const ensureConfirmed = useCallback(async () => {
    if (confirmedGlobal) {
      return true;
    }
    if (pendingConfirmation) {
      return pendingConfirmation;
    }
    const pending = new Promise<boolean>((resolve) => {
      Modal.confirm({
        title: CONFIRM_TITLE,
        content: CONFIRM_BODY,
        okText: 'Confirm and control',
        cancelText: 'Cancel',
        onOk: () => {
          confirmedGlobal = true;
          setConfirmed(true);
          resolve(true);
        },
        onCancel: () => resolve(false)
      });
    }).finally(() => {
      pendingConfirmation = null;
    });
    pendingConfirmation = pending;
    return pending;
  }, [setConfirmed]);

  return { confirmed, ensureConfirmed };
}
