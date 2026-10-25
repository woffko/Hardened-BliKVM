const PICOCLAW_SESSION_ID_KEY = 'hardened-blikvm-picoclaw-session-id';
const PICOCLAW_MAX_STEPS_KEY = 'hardened-blikvm-picoclaw-max-steps';
const PICOCLAW_MAX_RUNTIME_KEY = 'hardened-blikvm-picoclaw-max-runtime-ms';
const PICOCLAW_RUNTIME_INSTALL_SNAPSHOT_KEY = 'hardened-blikvm-picoclaw-runtime-install-snapshot';

export type PicoclawRuntimeInstallSnapshot = {
  installing: boolean;
  installProgress?: number;
  installStage?: string;
  status?: string;
};

function getSessionStore() {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

function getLocalValue(key: string) {
  return localStorage.getItem(key);
}

function setLocalValue(key: string, value: string) {
  localStorage.setItem(key, value);
}

function removeLocalValue(key: string) {
  localStorage.removeItem(key);
}

export function getPicoclawSessionId() {
  const sessionStore = getSessionStore();
  return sessionStore?.getItem(PICOCLAW_SESSION_ID_KEY) || getLocalValue(PICOCLAW_SESSION_ID_KEY);
}

export function setPicoclawSessionId(sessionId: string) {
  const sessionStore = getSessionStore();
  sessionStore?.setItem(PICOCLAW_SESSION_ID_KEY, sessionId);
  setLocalValue(PICOCLAW_SESSION_ID_KEY, sessionId);
}

export function clearPicoclawSessionId() {
  const sessionStore = getSessionStore();
  sessionStore?.removeItem(PICOCLAW_SESSION_ID_KEY);
  removeLocalValue(PICOCLAW_SESSION_ID_KEY);
}

/** Forgets everything the page keeps for the assistant, as a logout does. */
export function clearPicoclawStorage() {
  clearPicoclawSessionId();
  for (const key of [
    PICOCLAW_MAX_STEPS_KEY,
    PICOCLAW_MAX_RUNTIME_KEY,
    PICOCLAW_RUNTIME_INSTALL_SNAPSHOT_KEY
  ]) {
    removeLocalValue(key);
  }
}

export function getPicoclawMaxSteps() {
  const value = getLocalValue(PICOCLAW_MAX_STEPS_KEY);
  const parsed = value ? Number(value) : 20;
  return Math.max(1, Math.min(50, Number.isFinite(parsed) ? parsed : 20));
}

export function setPicoclawMaxSteps(maxSteps: number) {
  const value = Math.max(1, Math.min(50, Math.round(maxSteps)));
  setLocalValue(PICOCLAW_MAX_STEPS_KEY, String(value));
}

export function getPicoclawMaxRuntimeMs() {
  const value = getLocalValue(PICOCLAW_MAX_RUNTIME_KEY);
  const parsed = value ? Number(value) : 120000;
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return 120000;
  }
  return parsed;
}

export function setPicoclawMaxRuntimeMs(maxRuntimeMs: number) {
  const value = Math.max(1000, Math.round(maxRuntimeMs));
  setLocalValue(PICOCLAW_MAX_RUNTIME_KEY, String(value));
}

export function getPicoclawRuntimeInstallSnapshot(): PicoclawRuntimeInstallSnapshot | null {
  const value = getLocalValue(PICOCLAW_RUNTIME_INSTALL_SNAPSHOT_KEY);
  if (!value) {
    return null;
  }

  try {
    const parsed = JSON.parse(value) as PicoclawRuntimeInstallSnapshot;
    if (!parsed || typeof parsed !== 'object') {
      return null;
    }
    return {
      installing: parsed.installing === true,
      installProgress:
        typeof parsed.installProgress === 'number' ? parsed.installProgress : undefined,
      installStage: typeof parsed.installStage === 'string' ? parsed.installStage : undefined,
      status: typeof parsed.status === 'string' ? parsed.status : undefined
    };
  } catch {
    return null;
  }
}

export function setPicoclawRuntimeInstallSnapshot(snapshot: PicoclawRuntimeInstallSnapshot) {
  setLocalValue(PICOCLAW_RUNTIME_INSTALL_SNAPSHOT_KEY, JSON.stringify(snapshot));
}

export function clearPicoclawRuntimeInstallSnapshot() {
  removeLocalValue(PICOCLAW_RUNTIME_INSTALL_SNAPSHOT_KEY);
}
