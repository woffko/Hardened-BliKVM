import type { AgentMonitor } from '@/lib/agent-layout.ts';
import { http } from '@/lib/http.ts';

/** The target's system, as the agent's hello says it. */
export type AgentSystem = {
  product: string;
  release: string;
  build: number;
  revision: number;
  architecture: string;
  locked: boolean;
};

export type AgentLayout = { id: string; language: string; name: string };

/** The layout of the window in front, and those installed. */
export type AgentKeyboard = { current: AgentLayout | null; installed: AgentLayout[] };

/** What the agent answers: `"pong"`, or one read. */
export type AgentAnswer =
  | 'pong'
  | { system: AgentSystem }
  | { monitors: AgentMonitor[] }
  | { keyboard: AgentKeyboard };

/** The agent on the target, as the board says it. */
export type TargetAgentState = {
  /** The board keeps the agent's switch. */
  available: boolean;
  /** The operator's switch. */
  enabled: boolean;
  /** The channel's device is open on the board. */
  open: boolean;
  /** Why the channel's device is not open while it should be. */
  problem: string | null;
  /** The connected agent. */
  agent: {
    program: string;
    version: string;
    commit: string;
    system: AgentSystem | null;
  } | null;
  /** Why the agent's last hello was refused. */
  refused: string | null;
  messagesIn: number;
  messagesOut: number;
  dropped: number;
};

export function getTargetAgent() {
  return http.get('/api/target-agent');
}

/**
 * One call to the connected agent. A read is made only under the keyboard's
 * and mouse's lease this session holds; without it the board refuses it.
 */
export function callTargetAgent(call: 'ping' | 'system' | 'monitors' | 'keyboard') {
  return http.post('/api/target-agent/call', { call });
}

/** Switching makes the board's USB device leave the target and come back. */
export function setTargetAgent(enabled: boolean) {
  return http.post('/api/target-agent', { enabled });
}
