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
let pendingResolve: ((confirmed: boolean) => void) | null = null;
let pendingModal: { destroy: () => void } | null = null;

/**
 * Requires an explicit operator confirmation before restricted HID control.
 * Power actions use their own action-specific confirmation.
 */
export function useFocusConfirmation() {
  const [confirmed, setConfirmed] = useAtom(focusConfirmedAtom);

  useEffect(() => {
    if (confirmed) {
      confirmedGlobal = true;
    }
  }, [confirmed]);

  // Input handlers are passive: only the explicit enable action may open the
  // confirmation modal. This also makes Cancel sticky across queued reports.
  const ensureConfirmed = useCallback(async () => confirmedGlobal, []);

  const requestConfirmation = useCallback(async () => {
    if (confirmedGlobal) return true;
    if (pendingConfirmation) {
      return pendingConfirmation;
    }
    const pending = new Promise<boolean>((resolve) => {
      pendingResolve = resolve;
      pendingModal = Modal.confirm({
        title: CONFIRM_TITLE,
        content: CONFIRM_BODY,
        okText: 'Confirm and control',
        cancelText: 'Cancel',
        onOk: () => {
          confirmedGlobal = true;
          setConfirmed(true);
          resolve(true);
        },
        onCancel: () => {
          confirmedGlobal = false;
          setConfirmed(false);
          resolve(false);
        }
      });
    }).finally(() => {
      if (pendingConfirmation === pending) {
        pendingConfirmation = null;
        pendingResolve = null;
        pendingModal = null;
      }
    });
    pendingConfirmation = pending;
    return pending;
  }, [setConfirmed]);

  const disableControl = useCallback(() => {
    confirmedGlobal = false;
    setConfirmed(false);
    pendingModal?.destroy();
    pendingResolve?.(false);
  }, [setConfirmed]);

  return { confirmed, ensureConfirmed, requestConfirmation, disableControl };
}
