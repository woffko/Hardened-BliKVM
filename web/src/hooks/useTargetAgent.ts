import { useEffect } from 'react';
import { useAtomValue, useSetAtom } from 'jotai';

import * as api from '@/api/target-agent.ts';
import type { AgentMonitor } from '@/lib/agent-layout.ts';
import { agentKeyboardAtom, agentMonitorsAtom, targetAgentAtom } from '@/jotai/target-agent.ts';
import { leaseTransportAtom } from '@/jotai/transport.ts';

const STATE_EVERY_MS = 5000;
const READS_EVERY_MS = 15000;

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
      const keyboard = await api.callTargetAgent('keyboard').catch(() => null);
      const layouts = keyboard?.code === 0 ? (keyboard.data?.answer as api.AgentAnswer) : null;
      if (!gone && layouts && typeof layouts === 'object' && 'keyboard' in layouts)
        setKeyboard(layouts.keyboard);
    };
    void read();
    const timer = window.setInterval(() => void read(), READS_EVERY_MS);
    return () => {
      gone = true;
      window.clearInterval(timer);
    };
  }, [connected, enabled, leased, setKeyboard, setMonitors]);
}
