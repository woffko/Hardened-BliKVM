import { useEffect, useState } from 'react';
import { Button, Modal, Switch } from 'antd';
import { useAtom, useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/target-agent.ts';
import { agentStartCommand, commandSteps, runBoxSteps } from '@/lib/agent-start.ts';
import { agentMonitorsAtom, targetAgentAtom } from '@/jotai/target-agent.ts';
import { useAgentDisplays } from '@/hooks/useAgentDisplays.ts';
import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';
import { useHidLease } from '@/hooks/useHidLease.ts';
import { AgentMonitorList } from '@/pages/desktop/agent-monitor-list.tsx';

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

  // The page's one state of the agent, which the page reads every 5 s
  // (useTargetAgent); a switch made here puts the board's answer in it.
  const [state, setState] = useAtom(targetAgentAtom);
  const [isLoading, setIsLoading] = useState(false);
  const [confirming, setConfirming] = useState<boolean | null>(null);
  const [error, setError] = useState('');
  const monitors = useAtomValue(agentMonitorsAtom);
  const displays = useAgentDisplays();
  const { confirmed: controlling } = useFocusConfirmation();
  const { reportHid, drained } = useHidLease();
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
    // The Run box's second counts from when Win+R has been sent, not from
    // when it was queued: a lease slow to come would let the line run into
    // another window.
    await drained();
    await new Promise((resolve) => window.setTimeout(resolve, RUN_BOX_MS));
    for (const step of commandSteps(agentStartCommand(digest))) press(step.modifier, step.usage);
    setStarting('waiting');
  }

  // The agent connects (the board's own, when one of another version was
  // updated), or after a while the start is offered again with what to look
  // at on the target.
  const otherVersion = api.agentIsNotTheBoards(state);
  // The line runs only a program of the board's agent's digest; without it
  // nothing is typed.
  const digest = state?.agentSha256 ?? '';
  const digestKnown = /^[0-9a-f]{64}$/.test(digest);
  const agentConnected = Boolean(state?.agent) && !otherVersion;
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
        {otherVersion && (
          <span className="text-xs text-amber-400" data-testid="target-agent-other-version">
            {t('settings.device.targetAgent.otherVersion')}
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
          <AgentMonitorList
            monitors={monitors}
            className="text-neutral-400"
            testId="target-agent-monitors"
            disabled={readOnly || displays.busy}
            onChoose={(id) => void displays.chooseBoard(id)}
          />
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
        {enabled && (!state?.agent || otherVersion) && (
          <div className="flex flex-col space-y-1">
            <Button
              size="small"
              className="self-start"
              disabled={readOnly || !controlling || !digestKnown || starting !== null}
              onClick={() => setStarting('confirm')}
            >
              {otherVersion
                ? t('settings.device.targetAgent.update')
                : t('settings.device.targetAgent.start')}
            </Button>
            <span className="text-xs text-neutral-500">
              {!digestKnown
                ? t('settings.device.targetAgent.startNoDigest')
                : controlling
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
        title={
          otherVersion
            ? t('settings.device.targetAgent.update')
            : t('settings.device.targetAgent.start')
        }
        okText={t('settings.device.targetAgent.startConfirm')}
        cancelText={t('settings.device.cancelBtn')}
        onOk={() => void startAgent()}
        onCancel={() => setStarting(null)}
      >
        <p>
          {otherVersion
            ? t('settings.device.targetAgent.updateExplain')
            : t('settings.device.targetAgent.startExplain')}
        </p>
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
