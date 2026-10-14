import { atom } from 'jotai';

import type { AgentKeyboard, TargetAgentState } from '@/api/target-agent.ts';
import { areaFromMonitors, type AgentMonitor } from '@/lib/agent-layout.ts';

// The target agent's switch, channel and connected agent, as the board says
// them; null until read, or on a board without the agent.
export const targetAgentAtom = atom<TargetAgentState | null>(null);

// The connected agent's monitors and keyboard layouts, read under the
// keyboard's and mouse's lease; null until read, and again when the agent
// goes.
export const agentMonitorsAtom = atom<AgentMonitor[] | null>(null);
export const agentKeyboardAtom = atom<AgentKeyboard | null>(null);

// The absolute mouse's layout from the agent's monitors while it is
// connected and one of them is the board's (docs/design/target-agent-v1.md).
export const agentScreenAreaAtom = atom((get) => {
  const monitors = get(agentMonitorsAtom);
  return get(targetAgentAtom)?.agent && monitors ? areaFromMonitors(monitors) : null;
});
