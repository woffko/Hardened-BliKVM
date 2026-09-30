/** The owner's answer to a control command: a rejected one still arrives as a
 * successful HTTP call, with `applied: false` and the reason. */
export type CommandAnswer = {
  code: number;
  msg?: string;
  data?: { applied?: boolean; rejection?: string | null } | null;
};

/** Whether a command did not take effect: an API error, or an owner that
 * refused it. */
export function commandFailed(answer: CommandAnswer): boolean {
  return answer.code !== 0 || answer.data?.applied === false;
}

/** The reason to show for a failed command. */
export function commandReason(answer: CommandAnswer): string {
  return answer.msg || answer.data?.rejection || '';
}
