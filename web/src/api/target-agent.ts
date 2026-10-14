import { http } from '@/lib/http.ts';

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
  agent: { program: string; version: string; commit: string } | null;
  /** Why the agent's last hello was refused. */
  refused: string | null;
  messagesIn: number;
  messagesOut: number;
  dropped: number;
};

export function getTargetAgent() {
  return http.get('/api/target-agent');
}

/** Switching makes the board's USB device leave the target and come back. */
export function setTargetAgent(enabled: boolean) {
  return http.post('/api/target-agent', { enabled });
}
