import { useEffect } from 'react';
import { getDefaultStore, useAtomValue, useSetAtom } from 'jotai';

import * as api from '@/api/target-agent.ts';
import type { AgentMonitor } from '@/lib/agent-layout.ts';
import {
  agentKeyboardAtom,
  agentLayoutSwitchesAtom,
  agentMonitorsAtom,
  targetAgentAtom
} from '@/jotai/target-agent.ts';
import { leaseTransportAtom } from '@/jotai/transport.ts';

const STATE_EVERY_MS = 5000;
const READS_EVERY_MS = 15000;

/** Says in the page's state that the target's layout was switched to `layout`. */
export function layoutSwitched(layout: api.AgentLayout) {
  const store = getDefaultStore();
  store.set(agentLayoutSwitchesAtom, store.get(agentLayoutSwitchesAtom) + 1);
  const keyboard = store.get(agentKeyboardAtom);
  if (keyboard) store.set(agentKeyboardAtom, { ...keyboard, current: layout });
}

/**
 * Switches the target's layout to `layout` under the keyboard's and
 * mouse's lease (`hold`, then what is queued `drained`), as the typing
 * paths do, and says so in the page's state.
 */
export async function switchTargetLayout(
  layout: api.AgentLayout,
  hold: () => void,
  drained: () => Promise<void>
) {
  hold();
  await drained();
  await api.askTargetAgent({ set_layout: { id: layout.id } });
  layoutSwitched(layout);
}

/**
 * The target's layouts read now, and kept unless `keep` says otherwise or
 * the page switched the layout while the read was on its way: the agent may
 * have read them before the switch.
 */
export async function readAgentKeyboard(keep: () => boolean = () => true) {
  const store = getDefaultStore();
  const switches = store.get(agentLayoutSwitchesAtom);
  const answer = await api.askTargetAgent('keyboard');
  if (
    keep() &&
    store.get(agentLayoutSwitchesAtom) === switches &&
    typeof answer === 'object' &&
    'keyboard' in answer
  )
    store.set(agentKeyboardAtom, answer.keyboard);
}

/**
 * Keeps the target agent's state, and while an agent is connected and this
 * page holds the keyboard's and mouse's lease, its monitors and keyboard
 * layouts (docs/design/target-agent-v1.md). Mounted once, on a board that
 * offers the agent.
 */
export function useTargetAgent(enabled: boolean) {
  const setState = useSetAtom(targetAgentAtom);
  const setMonitors = useSetAtom(agentMonitorsAtom);
  const setKeyboard = useSetAtom(agentKeyboardAtom);
  const state = useAtomValue(targetAgentAtom);
  const leased = useAtomValue(leaseTransportAtom) !== null;
  const connected = Boolean(state?.agent);

  useEffect(() => {
    if (!enabled) return;
    let gone = false;
    const read = () =>
      api
        .getTargetAgent()
        .then((rsp) => {
          if (gone || rsp.code !== 0 || typeof rsp.data?.enabled !== 'boolean') return;
          setState(rsp.data as api.TargetAgentState);
        })
        .catch(() => undefined);
    read();
    const timer = window.setInterval(read, STATE_EVERY_MS);
    return () => {
      gone = true;
      window.clearInterval(timer);
    };
  }, [enabled, setState]);

  useEffect(() => {
    if (!connected) {
      setMonitors(null);
      setKeyboard(null);
    }
  }, [connected, setKeyboard, setMonitors]);

  useEffect(() => {
    if (!enabled || !connected || !leased) return;
    let gone = false;
    const read = async () => {
      const monitors = await api.callTargetAgent('monitors').catch(() => null);
      const answer = monitors?.code === 0 ? (monitors.data?.answer as api.AgentAnswer) : null;
      if (!gone && answer && typeof answer === 'object' && 'monitors' in answer)
        setMonitors(answer.monitors as AgentMonitor[]);
      await readAgentKeyboard(() => !gone).catch(() => undefined);
    };
    void read();
    const timer = window.setInterval(() => void read(), READS_EVERY_MS);
    return () => {
      gone = true;
      window.clearInterval(timer);
    };
  }, [connected, enabled, leased, setKeyboard, setMonitors]);
}
