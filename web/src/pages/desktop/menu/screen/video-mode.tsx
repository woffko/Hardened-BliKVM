import { useCallback, useEffect, useState } from 'react';
import { Tooltip } from 'antd';
import { useAtomValue } from 'jotai';
import { CheckIcon, TvMinimalPlayIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as firewallApi from '@/api/system-firewall.ts';
import {
  supportedUiVideoModes,
  supportsBoardFeature,
  type UiVideoMode
} from '@/lib/capabilities.ts';
import { setVideoMode as setCookie } from '@/lib/localstorage.ts';
import { browserSupportsWebRtc } from '@/lib/video-transport.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { videoModeAtom } from '@/jotai/screen.ts';
import { useQuicVideo } from '@/hooks/useQuicVideo.ts';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

// Both WebRTC transports carry media over UDP, which Restricted and Paranoid
// firewall modes drop.
const WEBRTC_MODES = ['h264', 'vp8'];

const videoModes = [
  { key: 'direct', name: 'H.264 (Direct)' },
  { key: 'h264', name: 'H.264 (WebRTC)' },
  { key: 'vp8', name: 'VP8 (WebRTC)' },
  { key: 'mjpeg', name: 'MJPEG' }
];

export const VideoMode = () => {
  const { t } = useTranslation();
  const capabilities = useAtomValue(capabilitiesAtom);
  const videoMode = useAtomValue(videoModeAtom);
  const supportedModes = supportedUiVideoModes(capabilities);
  // H.264 comes over the QUIC session when this browser uses QUIC and can
  // have the picture there; the entry says which of the two serves it.
  const h264OverQuic = useQuicVideo().offered;

  const [isDirectSupported, setIsDirectSupported] = useState<boolean | null>(null);
  const [isWebrtcBlocked, setIsWebrtcBlocked] = useState(false);
  // The firewall mode admits the QUIC transport's port although it closes
  // WebRTC's: Restricted does.
  const [isQuicAdmitted, setIsQuicAdmitted] = useState(false);
  // A VPN or privacy extension, or a policy, can switch WebRTC off in the
  // browser; both WebRTC modes then stay unselectable like under the firewall.
  const isWebrtcMissing = !browserSupportsWebRtc();

  const refreshFirewallStatus = useCallback(() => {
    firewallApi
      .getStatus()
      .then((rsp) => {
        if (rsp.code !== 0 || !rsp.data) return;
        const firewall = rsp.data;
        setIsWebrtcBlocked(
          firewall.webrtcBlocked ||
            firewall.restrictedActive ||
            firewall.paranoidActive ||
            firewall.effectiveMode === 'restricted' ||
            firewall.effectiveMode === 'paranoid'
        );
        setIsQuicAdmitted(firewall.quicBlocked === false);
      })
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    const isHttps = window.location.protocol === 'https:';
    const isDecoderSupported = !!window.VideoDecoder;

    setIsDirectSupported(isHttps && isDecoderSupported);
  }, []);

  useEffect(() => {
    refreshFirewallStatus();
  }, [refreshFirewallStatus]);

  useEffect(() => {
    function refreshIfVisible() {
      if (!document.hidden) {
        refreshFirewallStatus();
      }
    }

    window.addEventListener('focus', refreshFirewallStatus);
    document.addEventListener('visibilitychange', refreshIfVisible);

    return () => {
      window.removeEventListener('focus', refreshFirewallStatus);
      document.removeEventListener('visibilitychange', refreshIfVisible);
    };
  }, [refreshFirewallStatus]);

  useEffect(() => {
    if (supportedModes.includes(videoMode as UiVideoMode)) return;
    setCookie(supportedModes[0]);
    setTimeout(() => window.location.reload(), 250);
  }, [supportedModes, videoMode]);

  // A WebRTC mode cannot be had where WebRTC is off. Where the firewall mode
  // closes it, H.264 still can, while the QUIC session serves it. A browser
  // without WebRTC keeps both modes closed: the page's start takes it to
  // MJPEG before a session exists.
  const isUnavailable = (mode: string) =>
    WEBRTC_MODES.includes(mode) &&
    (isWebrtcMissing || (isWebrtcBlocked && !(mode === 'h264' && h264OverQuic)));
  const currentIsUnavailable = isUnavailable(videoMode);

  useEffect(() => {
    if (!currentIsUnavailable || isDirectSupported === null) return;

    setCookie(isDirectSupported && supportedModes.includes('direct') ? 'direct' : 'mjpeg');
    setTimeout(() => {
      window.location.reload();
    }, 250);
  }, [currentIsUnavailable, isDirectSupported, supportedModes]);

  // H.264 would come over QUIC here, were QUIC this browser's choice.
  const quicWouldServe =
    isWebrtcBlocked &&
    isQuicAdmitted &&
    typeof WebTransport !== 'undefined' &&
    supportsBoardFeature(capabilities, 'network_quic');

  function update(mode: string) {
    if (mode === videoMode) return;
    if (!supportedModes.includes(mode as UiVideoMode)) return;
    if (isUnavailable(mode)) return;

    setCookie(mode);

    // reload after changing video mode
    setTimeout(() => {
      window.location.reload();
    }, 500);
  }

  const content = (
    <>
      {supportedModes.includes('direct') && !isDirectSupported && (
        <Tooltip
          title={t('screen.videoDirectTips')}
          placement="right"
          styles={{ root: { maxWidth: '270px' } }}
        >
          <div className="flex cursor-not-allowed select-none items-center rounded py-1.5 pl-1 pr-5 text-neutral-500 hover:bg-neutral-700/70">
            <div className="flex h-[14px] w-[20px] items-end text-blue-500"></div>
            <span>H.264 (Direct)</span>
          </div>
        </Tooltip>
      )}

      {videoModes.map(
        (mode) =>
          supportedModes.includes(mode.key as UiVideoMode) &&
          (isDirectSupported || mode.key !== 'direct') && (
            <Tooltip
              key={mode.key}
              title={
                !isUnavailable(mode.key)
                  ? undefined
                  : isWebrtcMissing
                    ? t('screen.videoWebrtcUnavailable')
                    : t(
                        mode.key === 'vp8'
                          ? 'screen.videoVp8BlockedByFirewall'
                          : quicWouldServe
                            ? 'screen.videoWebrtcBlockedUseQuic'
                            : 'screen.videoWebrtcBlockedByFirewall'
                      )
              }
              placement="right"
              styles={{ root: { maxWidth: '270px' } }}
            >
              <div
                aria-disabled={isUnavailable(mode.key)}
                data-video-mode={mode.key}
                className={
                  isUnavailable(mode.key)
                    ? 'flex cursor-not-allowed select-none items-center rounded py-1.5 pl-1 pr-5 text-neutral-500 hover:bg-neutral-700/70'
                    : 'flex cursor-pointer select-none items-center rounded py-1.5 pl-1 pr-5 hover:bg-neutral-700/70'
                }
                onClick={() => update(mode.key)}
              >
                <div className="flex h-[14px] w-[20px] items-end text-blue-500">
                  {mode.key === videoMode && <CheckIcon size={15} />}
                </div>
                <span data-video-mode-name={mode.key}>
                  {mode.key === 'h264' && h264OverQuic ? 'H.264 (QUIC)' : mode.name}
                </span>
              </div>
            </Tooltip>
          )
      )}
    </>
  );

  return (
    <MenuSubmenu
      icon={<TvMinimalPlayIcon size={18} />}
      label={t('screen.video')}
      content={content}
      onOpenChange={(open) => {
        if (open) refreshFirewallStatus();
      }}
    />
  );
};
