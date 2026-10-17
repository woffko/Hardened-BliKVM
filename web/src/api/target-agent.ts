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

/** What the agent answers: `"pong"`, `"done"`, or one read. */
export type AgentAnswer =
  | 'pong'
  | 'done'
  | { system: AgentSystem }
  | { monitors: AgentMonitor[] }
  | { keyboard: AgentKeyboard }
  | { displays_changed: { revert_in_seconds: number } }
  | { clipboard: string };

/** The monitors' modes the agent sets. */
export type DisplayMode = 'extend' | 'duplicate' | 'board_only' | 'others_only' | 'board_primary';

/** What the page may ask the agent. */
export type AgentCall =
  | 'ping'
  | 'system'
  | 'monitors'
  | 'keyboard'
  | 'keep_displays'
  | 'read_clipboard'
  | 'lock'
  | { set_layout: { id: string } }
  | { set_displays: { mode: DisplayMode } }
  | { write_clipboard: { text: string } }
  | { set_board_monitor: { id: string | null } };

/** The most clipboard text either way, in UTF-8 bytes. */
export const MAX_CLIPBOARD_BYTES = 6000;

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
  /** The commit the board was built from, which its hello names to the agent; empty or
   * `unknown` from a board that does not say. */
  boardCommit?: string;
};

/** Whether the connected agent was built from another commit than the board: the page
 * then offers to update it from the board's drive. A board that does not name its commit
 * is not said to differ. */
export function agentIsNotTheBoards(state: TargetAgentState | null): boolean {
  const board = state?.boardCommit ?? '';
  return (
    Boolean(state?.agent) && board !== '' && board !== 'unknown' && state?.agent?.commit !== board
  );
}

export function getTargetAgent() {
  return http.get('/api/target-agent');
}

/**
 * One call to the connected agent. A read is made only under the keyboard's
 * and mouse's lease this session holds; without it the board refuses it.
 */
export function callTargetAgent(call: AgentCall) {
  return http.post('/api/target-agent/call', { call });
}

/** The agent's answer to `call` and its round trip, or the reason there is none. */
export async function askTargetAgentTimed(
  call: AgentCall
): Promise<{ answer: AgentAnswer; roundTripUs: number }> {
  const rsp = await callTargetAgent(call);
  if (rsp.code !== 0 || rsp.data?.answer === undefined)
    throw new Error(rsp.msg || 'the agent did not answer');
  return {
    answer: rsp.data.answer as AgentAnswer,
    roundTripUs: typeof rsp.data.roundTripUs === 'number' ? rsp.data.roundTripUs : 0
  };
}

/** The agent's answer to `call`, or the reason there is none. */
export async function askTargetAgent(call: AgentCall): Promise<AgentAnswer> {
  return (await askTargetAgentTimed(call)).answer;
}

/** Switching makes the board's USB device leave the target and come back. */
export function setTargetAgent(enabled: boolean) {
  return http.post('/api/target-agent', { enabled });
}
