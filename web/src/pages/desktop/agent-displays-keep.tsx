import { useEffect, useState } from 'react';
import { Modal } from 'antd';
import { useAtom } from 'jotai';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/target-agent.ts';
import { displaysRevertAtAtom } from '@/jotai/target-agent.ts';
import { DISPLAYS_SETTLE_MS, readAgentMonitors } from '@/hooks/useAgentDisplays.ts';
import { useHidLease } from '@/hooks/useHidLease.ts';

/**
 * The one dialog that keeps a monitors' change, wherever it was made: it
 * counts down to the agent's putting it back, keeps it when asked (under the
 * keyboard's and mouse's lease, as every call to the agent is), and reads
 * the monitors again once the change is kept or put back, so that the
 * mouse follows. It stays until the agent has kept the change: a Keep that
 * failed is said, the countdown goes on, and Keep may be pressed again.
 */
export const AgentDisplaysKeep = () => {
  const { t } = useTranslation();
  const [revertAt, setRevertAt] = useAtom(displaysRevertAtAtom);
  const [left, setLeft] = useState(0);
  const [keeping, setKeeping] = useState(false);
  const [failure, setFailure] = useState('');
  const { hold, drained } = useHidLease();

  useEffect(() => {
    if (revertAt === null) return;
    const tick = () => {
      const seconds = Math.ceil((revertAt - performance.now()) / 1000);
      if (seconds > 0) {
        setLeft(seconds);
        return;
      }
      setRevertAt(null);
      window.setTimeout(() => void readAgentMonitors().catch(() => undefined), DISPLAYS_SETTLE_MS);
    };
    tick();
    const timer = window.setInterval(tick, 250);
    return () => window.clearInterval(timer);
  }, [revertAt, setRevertAt]);

  // A new change, or none waiting: no failure is left from before.
  useEffect(() => setFailure(''), [revertAt]);

  async function keep() {
    setKeeping(true);
    setFailure('');
    try {
      hold();
      await drained();
      await api.askTargetAgent('keep_displays');
      setRevertAt(null);
      await readAgentMonitors().catch(() => undefined);
    } catch (err) {
      setFailure(err instanceof Error ? err.message : String(err));
    } finally {
      setKeeping(false);
    }
  }

  // Not kept: the agent puts the monitors back when the time is up.
  function dontKeep() {
    const wait = revertAt === null ? 0 : Math.max(0, revertAt - performance.now());
    setRevertAt(null);
    window.setTimeout(
      () => void readAgentMonitors().catch(() => undefined),
      wait + DISPLAYS_SETTLE_MS
    );
  }

  return (
    <Modal
      open={revertAt !== null}
      title={t('settings.device.targetAgent.displays.keepTitle')}
      okText={t('settings.device.targetAgent.displays.keep')}
      cancelText={t('settings.device.targetAgent.displays.putBack')}
      okButtonProps={{ disabled: keeping }}
      onOk={() => void keep()}
      onCancel={dontKeep}
    >
      <p>{t('settings.device.targetAgent.displays.keepLeft', { seconds: left })}</p>
      {failure && (
        <p role="alert" className="text-red-500" data-testid="agent-displays-keep-failure">
          {t('settings.device.targetAgent.displays.keepFailed', { reason: failure })}
        </p>
      )}
    </Modal>
  );
};
