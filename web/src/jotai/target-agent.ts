import { atom } from 'jotai';

import type { AgentKeyboard, TargetAgentState } from '@/api/target-agent.ts';
import { areaFromMonitors, type AgentMonitor } from '@/lib/agent-layout.ts';
import { getAgentLayoutSync, setAgentLayoutSync } from '@/lib/localstorage.ts';

// The target agent's switch, channel and connected agent, as the board says
// them; null until read, or on a board without the agent.
export const targetAgentAtom = atom<TargetAgentState | null>(null);

// The connected agent's monitors and keyboard layouts, read under the
// keyboard's and mouse's lease; null until read, and again when the agent
// goes.
export const agentMonitorsAtom = atom<AgentMonitor[] | null>(null);
export const agentKeyboardAtom = atom<AgentKeyboard | null>(null);

// A monitors' change the agent puts back unless it is kept: when, on this
// page's monotonic clock (performance.now()); null while none waits.
export const displaysRevertAtAtom = atom<number | null>(null);

// How long the last monitors' change took the agent, in milliseconds, from
// its call's round trip; the Keep dialog says it.
export const displaysChangeTookAtom = atom<number | null>(null);

// Keep the target's layout in step with the on-screen keyboard's language
// (this browser's choice).
const layoutSyncAtom = atom(getAgentLayoutSync());
export const agentLayoutSyncAtom = atom(
  (get) => get(layoutSyncAtom),
  (_get, set, sync: boolean) => {
    set(layoutSyncAtom, sync);
    setAgentLayoutSync(sync);
  }
);

// The absolute mouse's layout from the agent's monitors while it is
// connected and one of them is the board's (docs/design/target-agent-v1.md).
export const agentScreenAreaAtom = atom((get) => {
  const monitors = get(agentMonitorsAtom);
  return get(targetAgentAtom)?.agent && monitors ? areaFromMonitors(monitors) : null;
});
