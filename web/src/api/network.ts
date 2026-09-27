import { http } from '@/lib/http.ts';

export type DNSMode = 'manual' | 'dhcp';
export type IPv6Mode = 'disabled' | 'slaac' | 'dhcpv6' | 'manual';

export type NetworkConfig = {
  interface?: string;
  address?: string;
  subnetMask?: string;
  gateway?: string;
};

export type IPv6Config = {
  interface?: string;
  address?: string;
  prefix?: number;
  gateway?: string;
};

// wake on lan
export function wol(mac: string) {
  // Pressing Ok in the Wake-on-LAN dialog is the explicit confirmation the
  // hardened network write requires.
  const data = {
    mac,
    confirm: true
  };
  return http.post('/api/network/wol', data);
}

// get wake-on-lan macs history
export function getWolMacs() {
  return http.get('/api/network/wol/mac');
}

export function deleteWolMac(mac: string) {
  const data = {
    mac
  };
  return http.delete('/api/network/wol/mac', data);
}

// set Mac name
export function setWolMacName(mac: string, name: string) {
  return http.post('/api/network/wol/mac/name', { mac, name });
}

// get wifi information
export function getWiFi() {
  return http.get('/api/network/wifi');
}

// connect wifi without auth (only available in wifi configuration mode)
export function connectWifiNoAuth(ssid: string, password: string, apPassword?: string) {
  const data = {
    ssid,
    password
  };
  return http.post('/api/network/wifi', data, {
    headers: {
      'X-AP-Key': apPassword || ''
    }
  });
}

// verify ap login
export function verifyApLogin(apPassword: string) {
  return http.post(
    '/api/network/wifi/verify',
    {},
    {
      headers: {
        'X-AP-Key': apPassword || ''
      }
    }
  );
}

export type WifiSecurity = 'open' | 'wpa2' | 'wpa3' | 'wpa2_wpa3' | 'wep' | 'enterprise' | 'other';

export type WifiStatus = {
  available: boolean;
  connected: boolean;
  ssid: string;
  interface?: string;
  state: string;
  radio: boolean;
  signal: number | null;
  security: WifiSecurity | null;
  address: string | null;
  gateway: string | null;
  dns: string[];
  ipv6: string | null;
  hwAddress: string | null;
  channel: number | null;
  rateMbps: number | null;
  wifiPreferred: boolean;
  revertInSeconds: number;
};

export type WifiNetwork = {
  ssid: string;
  signal: number;
  security: WifiSecurity;
  channel: number;
  inUse: boolean;
};

export type WifiProfile = {
  id: string;
  ssid: string;
  autoconnect: boolean;
  active: boolean;
};

export type WifiProfileAction = 'connect' | 'forget' | 'autoconnect_on' | 'autoconnect_off';

// the networks in range; rescan asks the board for a new scan first
export function scanWifi(rescan: boolean) {
  return http.get('/api/network/wifi/scan', { rescan });
}

// the saved networks
export function getSavedWifi() {
  return http.get('/api/network/wifi/saved');
}

// act on a saved network
export function wifiProfile(id: string, action: WifiProfileAction) {
  return http.post('/api/network/wifi/saved', { id, action });
}

// connect wifi; an association can take up to 30 seconds
export function connectWifi(ssid: string, password: string, hidden = false) {
  return http.post('/api/network/wifi/connect', { ssid, password, hidden });
}

// disconnect wifi
export function disconnectWifi() {
  return http.post('/api/network/wifi/disconnect');
}

// make wifi (or ethernet) the main connection; reverted unless confirmed
export function setWifiPreferred(enabled: boolean) {
  return http.post('/api/network/wifi/preferred', { enabled });
}

export function confirmWifiPreferred() {
  return http.post('/api/network/wifi/preferred/confirm');
}

// switch the wifi radio
export function setWifiRadio(enabled: boolean) {
  return http.post('/api/network/wifi/radio', { enabled });
}

export function getDNS() {
  return http.get('/api/network/dns');
}

export function setDNS(mode: DNSMode, servers: string[], config?: NetworkConfig) {
  return http.post('/api/network/dns', { mode, servers, ...(config || {}) });
}

// keep an addressing change that would otherwise revert
export function confirmNetwork() {
  return http.post('/api/network/dns/confirm');
}

export function getIPv6() {
  return http.get('/api/network/ipv6');
}

export function setIPv6(mode: IPv6Mode, config?: IPv6Config) {
  return http.post('/api/network/ipv6', { mode, ...(config || {}) });
}
