import { getAccount } from '@/api/auth.ts';

/**
 * Confirms the session before a stream or socket reconnects. A 401 makes the
 * HTTP client reload into the login page; other failures (the server is still
 * restarting) report the session as usable so the caller simply retries.
 */
export async function checkSession(): Promise<boolean> {
  try {
    await getAccount();
    return true;
  } catch (error) {
    const status = (error as { response?: { status?: number } }).response?.status;
    return status !== 401;
  }
}
