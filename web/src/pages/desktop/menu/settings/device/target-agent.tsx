import { useCallback, useEffect, useState } from 'react';
import { Modal, Switch } from 'antd';
import { useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/target-agent.ts';
import { agentMonitorsAtom } from '@/jotai/target-agent.ts';

/** The agent's name, version and the first twelve characters of its commit. */
function agentTitle(agent: NonNullable<api.TargetAgentState['agent']>) {
  return `${agent.program} ${agent.version} (${agent.commit.slice(0, 12)})`;
}

export const TargetAgent = ({ readOnly = false }: { readOnly?: boolean }) => {
  const { t } = useTranslation();

  const [state, setState] = useState<api.TargetAgentState | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [confirming, setConfirming] = useState<boolean | null>(null);
  const [error, setError] = useState('');
  const monitors = useAtomValue(agentMonitorsAtom);

  const read = useCallback(async () => {
    const rsp = await api.getTargetAgent();
    if (rsp.code !== 0 || typeof rsp.data?.enabled !== 'boolean')
      throw new Error(rsp.msg || t('settings.device.targetAgent.readFailed'));
    setState(rsp.data as api.TargetAgentState);
  }, [t]);

  useEffect(() => {
    let cancelled = false;
    const refresh = () =>
      read()
        .catch((err: unknown) => {
          if (!cancelled) setError(err instanceof Error ? err.message : String(err));
        })
        .finally(() => {
          if (!cancelled) setIsLoading(false);
        });
    refresh();
    // The agent connects and goes on its own: the state is read again.
    const timer = window.setInterval(refresh, 5000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [read]);

  async function update(enabled: boolean) {
    setConfirming(null);
    setIsLoading(true);
    setError('');
    try {
      const rsp = await api.setTargetAgent(enabled);
      if (rsp.code !== 0 || typeof rsp.data?.enabled !== 'boolean')
        throw new Error(rsp.msg || t('settings.device.targetAgent.switchFailed'));
      setState(rsp.data as api.TargetAgentState);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setIsLoading(false);
    }
  }

  const enabled = state?.enabled ?? false;
  const system = state?.agent?.system ?? null;
  let status = '';
  if (state?.agent) {
    status = t('settings.device.targetAgent.connected', { agent: agentTitle(state.agent) });
  } else if (enabled && state?.problem) {
    status = t('settings.device.targetAgent.problem', { problem: state.problem });
  } else if (enabled && state?.refused) {
    status = t('settings.device.targetAgent.refused', { reason: state.refused });
  } else if (enabled) {
    status = t('settings.device.targetAgent.waiting');
  }

  return (
    <div className="flex items-center justify-between">
      <div className="flex flex-col space-y-1">
        <span>{t('settings.device.targetAgent.title')}</span>
        <span className="text-xs text-neutral-500">
          {t('settings.device.targetAgent.description')}
        </span>
        {status && (
          <span className="text-xs text-neutral-400" data-testid="target-agent-status">
            {status}
          </span>
        )}
        {system && (
          <span className="text-xs text-neutral-400" data-testid="target-agent-system">
            {t('settings.device.targetAgent.system', {
              product: system.product,
              release: system.release,
              build: `${system.build}.${system.revision}`
            })}
            {system.locked ? ` — ${t('settings.device.targetAgent.locked')}` : ''}
          </span>
        )}
        {state?.agent && monitors && (
          <ul className="text-xs text-neutral-400" data-testid="target-agent-monitors">
            {monitors.map((monitor) => (
              <li key={`${monitor.x},${monitor.y}`}>
                {t('settings.device.targetAgent.monitor', {
                  name: monitor.name || monitor.maker,
                  width: monitor.width,
                  height: monitor.height,
                  x: monitor.x,
                  y: monitor.y
                })}
                {monitor.board ? ` — ${t('settings.device.targetAgent.boards')}` : ''}
              </li>
            ))}
          </ul>
        )}
        {state?.agent && !monitors && (
          <span className="text-xs text-neutral-500">
            {t('settings.device.targetAgent.monitorsUnderControl')}
          </span>
        )}
        {error && (
          <span role="alert" className="text-xs text-red-400">
            {error}
          </span>
        )}
      </div>

      <Switch
        aria-label={t('settings.device.targetAgent.title')}
        checked={enabled}
        loading={isLoading}
        disabled={readOnly || !state?.available}
        onChange={(next) => setConfirming(next)}
      />

      <Modal
        open={confirming !== null}
        title={t('settings.device.targetAgent.title')}
        okText={t('settings.device.targetAgent.confirm')}
        cancelText={t('settings.device.cancelBtn')}
        onOk={() => confirming !== null && update(confirming)}
        onCancel={() => setConfirming(null)}
      >
        <p>
          {confirming
            ? t('settings.device.targetAgent.confirmOn')
            : t('settings.device.targetAgent.confirmOff')}
        </p>
      </Modal>
    </div>
  );
};
