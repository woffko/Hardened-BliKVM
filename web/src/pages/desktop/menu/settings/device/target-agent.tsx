import { useCallback, useEffect, useState } from 'react';
import { Button, Modal, Switch } from 'antd';
import { useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/target-agent.ts';
import { commandSteps, runBoxSteps } from '@/lib/agent-start.ts';
import { agentMonitorsAtom } from '@/jotai/target-agent.ts';
import { useAgentDisplays } from '@/hooks/useAgentDisplays.ts';
import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';
import { useHidLease } from '@/hooks/useHidLease.ts';

/** How long Windows' Run box is given to open before the line is typed. */
const RUN_BOX_MS = 1000;
/** How long the started agent is waited for before the start is offered again. */
const START_WAIT_MS = 20000;

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
  const displays = useAgentDisplays();
  const { confirmed: controlling } = useFocusConfirmation();
  const { reportHid } = useHidLease();
  const [starting, setStarting] = useState<'confirm' | 'typing' | 'waiting' | null>(null);

  // Win+R, then the line that starts the agent from its drive, then Enter;
  // the agent is then waited for.
  async function startAgent() {
    setStarting('typing');
    setError('');
    const press = (modifier: number, usage: number) => {
      reportHid({ mode: 'keyboard', data: [modifier, 0, usage, 0, 0, 0, 0, 0] });
      reportHid({ mode: 'keyboard', data: [0, 0, 0, 0, 0, 0, 0, 0] });
    };
    for (const step of runBoxSteps()) press(step.modifier, step.usage);
    await new Promise((resolve) => window.setTimeout(resolve, RUN_BOX_MS));
    for (const step of commandSteps()) press(step.modifier, step.usage);
    setStarting('waiting');
  }

  // The agent connects, or after a while the start is offered again with
  // what to look at on the target.
  const agentConnected = Boolean(state?.agent);
  useEffect(() => {
    if (starting !== 'waiting') return;
    if (agentConnected) {
      setStarting(null);
      return;
    }
    const timer = window.setTimeout(() => {
      setStarting(null);
      setError(t('settings.device.targetAgent.startTimeout'));
    }, START_WAIT_MS);
    return () => window.clearTimeout(timer);
  }, [agentConnected, starting, t]);
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
          <ul
            className="m-0 list-none p-0 text-xs text-neutral-400"
            data-testid="target-agent-monitors"
          >
            {monitors.map((monitor) => (
              <li
                key={monitor.id || `${monitor.x},${monitor.y}`}
                className="flex items-center justify-between gap-2"
              >
                <span className="min-w-0 flex-1">
                  {t('settings.device.targetAgent.monitor', {
                    name: monitor.name || monitor.maker,
                    width: monitor.width,
                    height: monitor.height,
                    x: monitor.x,
                    y: monitor.y
                  })}
                  {monitor.board ? ` — ${t('settings.device.targetAgent.boards')}` : ''}
                </span>
                {!monitor.board && monitor.id && (
                  <Button
                    size="small"
                    type="link"
                    className="h-auto shrink-0 px-0 text-xs"
                    disabled={readOnly || displays.busy}
                    onClick={() => void displays.chooseBoard(monitor.id!)}
                  >
                    {t('settings.device.targetAgent.chooseBoard')}
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
        {state?.agent && monitors && !monitors.some((monitor) => monitor.board) && (
          <span className="text-xs text-neutral-500" data-testid="target-agent-choose-board">
            {t('settings.device.targetAgent.chooseBoardHint')}
          </span>
        )}
        {state?.agent && !monitors && (
          <span className="text-xs text-neutral-500">
            {t('settings.device.targetAgent.monitorsUnderControl')}
          </span>
        )}
        {enabled && !state?.agent && (
          <div className="flex flex-col space-y-1">
            <Button
              size="small"
              className="self-start"
              disabled={readOnly || !controlling || starting !== null}
              onClick={() => setStarting('confirm')}
            >
              {t('settings.device.targetAgent.start')}
            </Button>
            <span className="text-xs text-neutral-500">
              {controlling
                ? starting === 'waiting'
                  ? t('settings.device.targetAgent.startWaiting')
                  : t('settings.device.targetAgent.startHint')
                : t('settings.device.targetAgent.startNeedsControl')}
            </span>
          </div>
        )}
        {state?.agent && monitors && (
          <div className="flex flex-wrap gap-1" data-testid="target-agent-displays">
            {(['extend', 'duplicate', 'board_only', 'others_only', 'board_primary'] as const).map(
              (mode) => (
                <Button
                  key={mode}
                  size="small"
                  disabled={readOnly || displays.busy || displays.keeping}
                  onClick={() => void displays.changeDisplays(mode)}
                >
                  {t(`settings.device.targetAgent.displays.${mode}`)}
                </Button>
              )
            )}
          </div>
        )}
        {(error || displays.error) && (
          <span role="alert" className="text-xs text-red-400">
            {error || displays.error}
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
        open={starting === 'confirm'}
        title={t('settings.device.targetAgent.start')}
        okText={t('settings.device.targetAgent.startConfirm')}
        cancelText={t('settings.device.cancelBtn')}
        onOk={() => void startAgent()}
        onCancel={() => setStarting(null)}
      >
        <p>{t('settings.device.targetAgent.startExplain')}</p>
      </Modal>

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
