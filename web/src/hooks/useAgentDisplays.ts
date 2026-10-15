import { useState } from 'react';
import { getDefaultStore, useAtomValue } from 'jotai';

import * as api from '@/api/target-agent.ts';
import type { AgentMonitor } from '@/lib/agent-layout.ts';
import { agentMonitorsAtom, displaysRevertAtAtom } from '@/jotai/target-agent.ts';

/** How long Windows is given to settle a monitors' change before they are read. */
export const DISPLAYS_SETTLE_MS = 1500;

/** The agent's monitors read now, for the page and the mouse's layout. */
export async function readAgentMonitors() {
  const answer = await api.askTargetAgent('monitors');
  if (typeof answer === 'object' && 'monitors' in answer)
    getDefaultStore().set(agentMonitorsAtom, answer.monitors as AgentMonitor[]);
}

const settle = () => new Promise((resolve) => window.setTimeout(resolve, DISPLAYS_SETTLE_MS));

/**
 * The monitors' mode and the board's monitor, as the operator changes them
 * from the Monitors menu or the settings (docs/design/target-agent-v1.md):
 * each a call to the agent under the HID lease, the monitors read again
 * after it so that the mouse follows. A mode change waits to be kept in the
 * page's one dialog (`AgentDisplaysKeep`), and the agent puts it back by
 * itself unless it is.
 */
export function useAgentDisplays() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const keeping = useAtomValue(displaysRevertAtAtom) !== null;

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError('');
    try {
      await action();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  const changeDisplays = (mode: api.DisplayMode) =>
    run(async () => {
      const answer = await api.askTargetAgent({ set_displays: { mode } });
      if (typeof answer === 'object' && 'displays_changed' in answer)
        getDefaultStore().set(
          displaysRevertAtAtom,
          performance.now() + answer.displays_changed.revert_in_seconds * 1000
        );
      await settle();
      await readAgentMonitors();
    });

  const chooseBoard = (id: string) =>
    run(async () => {
      await api.askTargetAgent({ set_board_monitor: { id } });
      await readAgentMonitors();
    });

  return { busy, error, keeping, changeDisplays, chooseBoard };
}
