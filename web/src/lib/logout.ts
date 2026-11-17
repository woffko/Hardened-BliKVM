import type { NavigateFunction } from 'react-router-dom';

import * as api from '@/api/auth.ts';
import { removeToken } from '@/lib/cookie.ts';
import { clearPicoclawStorage } from '@/lib/picoclaw-storage.ts';

/**
 * Ends the session on the board and goes to the login page, as Settings >
 * Account and the toolbar's button do. Returns whether the board ended it.
 */
export async function logOut(navigate: NavigateFunction): Promise<boolean> {
  const rsp = await api.logout();
  if (rsp.code !== 0) {
    console.log(rsp.msg);
    return false;
  }
  removeToken();
  clearPicoclawStorage();
  navigate('/auth/login');
  return true;
}
