import { useEffect, useState } from 'react';
import { Modal } from 'antd';
import { useAtom } from 'jotai';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/target-agent.ts';
import { displaysRevertAtAtom } from '@/jotai/target-agent.ts';
import { DISPLAYS_SETTLE_MS, readAgentMonitors } from '@/hooks/useAgentDisplays.ts';

/**
 * The one dialog that keeps a monitors' change, wherever it was made: it
 * counts down to the agent's putting it back, keeps it when asked, and reads
 * the monitors again once the change is kept or put back, so that the
 * mouse follows.
 */
export const AgentDisplaysKeep = () => {
  const { t } = useTranslation();
  const [revertAt, setRevertAt] = useAtom(displaysRevertAtAtom);
  const [left, setLeft] = useState(0);

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

  async function keep() {
    setRevertAt(null);
    await api.askTargetAgent('keep_displays').catch(() => undefined);
    await readAgentMonitors().catch(() => undefined);
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
      onOk={() => void keep()}
      onCancel={dontKeep}
    >
      <p>{t('settings.device.targetAgent.displays.keepLeft', { seconds: left })}</p>
    </Modal>
  );
};
