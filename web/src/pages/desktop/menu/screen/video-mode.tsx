import { useCallback, useEffect, useRef, useState } from 'react';
import { message, Tooltip } from 'antd';
import { useAtomValue } from 'jotai';
import { CheckIcon, TvMinimalPlayIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as firewallApi from '@/api/system-firewall.ts';
import { getScreen, updateScreen } from '@/api/vm';
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

type H264Engine = 'hardware' | 'software';

/** One entry of the menu: a mode, and for H.264 where the board lets its
 * engine be chosen, the engine. */
type Entry = { key: string; mode: string; engine?: H264Engine; name: string };

/**
 * The board's H.264 engine as `GET /api/vm/screen` reports it: the choice,
 * what encodes, and whether the video engine can be had
 * (docs/design/h264-engine-choice-and-quality-monitor-v1.md).
 */
type BoardEngine = {
  engine?: H264Engine;
  encoder?: 'hardware' | 'x264';
  available?: boolean;
  reason?: string;
};

function boardEngineOf(data: Record<string, unknown>): BoardEngine {
  return {
    engine:
      data.h264Engine === 'hardware' || data.h264Engine === 'software'
        ? data.h264Engine
        : undefined,
    encoder:
      data.h264Encoder === 'hardware' || data.h264Encoder === 'x264' ? data.h264Encoder : undefined,
    available:
      typeof data.h264HardwareAvailable === 'boolean' ? data.h264HardwareAvailable : undefined,
    reason:
      typeof data.h264HardwareReason === 'string' && data.h264HardwareReason
        ? data.h264HardwareReason.slice(0, 120)
        : undefined
  };
}

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

  // Where the board lets its H.264 engine be seen, the one H.264 entry is
  // two, one for each engine. The engine is the board's: every H.264 viewer
  // gets it. Without the right to change it, the entries only choose this
  // browser's mode.
  const offersEngine = (capabilities?.video.settings ?? []).includes('h264_engine');
  const mayChooseEngine = offersEngine && capabilities?.video.status.availability === 'read_write';
  const [board, setBoard] = useState<BoardEngine>({});
  const choosing = useRef(false);

  const refreshBoardEngine = useCallback(() => {
    if (!offersEngine) return;
    getScreen()
      .then((rsp) => {
        if (rsp.code !== 0 || !rsp.data) return;
        setBoard(boardEngineOf(rsp.data));
      })
      .catch(() => undefined);
  }, [offersEngine]);

  useEffect(() => {
    refreshBoardEngine();
  }, [refreshBoardEngine]);

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

  // The encoder has no engine to offer at all: its unit names no device, so
  // it gives no reason, and the entry cannot be chosen. Where it gives one
  // (busy, no device node, three frames in a row failed) the entry stays
  // choosable: the encoder tries the engine again only when the choice is
  // made again.
  const isEngineUnavailable = (entry: Entry) =>
    entry.engine === 'hardware' && board.available === false && !board.reason;

  // An H.264 entry with its engine: this browser's mode becomes H.264, and
  // the board's choice the entry's where it is another and may be changed.
  // A stream that is already H.264 changes its engine without a reload.
  async function choose(entry: Entry) {
    if (!entry.engine) return update(entry.mode);
    if (choosing.current) return;
    if (!supportedModes.includes('h264')) return;
    if (isUnavailable('h264') || isEngineUnavailable(entry)) return;
    const modeChanges = videoMode !== 'h264';
    // The board's choice is posted again where it is hardware and x264 encodes:
    // that is the retry of an engine that was busy or failed.
    const retry =
      entry.engine === 'hardware' && board.engine === 'hardware' && board.encoder === 'x264';
    if (!modeChanges && (!mayChooseEngine || (board.engine === entry.engine && !retry))) return;
    choosing.current = true;
    try {
      if (mayChooseEngine && (board.engine !== entry.engine || retry)) {
        const rsp = await updateScreen('h264_engine', entry.engine === 'hardware' ? 1 : 0);
        if (rsp.code !== 0 || rsp.data?.applied !== true || rsp.data?.rejection != null)
          throw new Error(rsp.data?.rejection ?? 'not applied');
        setBoard((current) => ({ ...current, engine: entry.engine }));
      }
      if (modeChanges) {
        setCookie('h264');
        setTimeout(() => {
          window.location.reload();
        }, 500);
      } else {
        // The encoder switches at its next frame: what encodes is read again.
        setTimeout(refreshBoardEngine, 1000);
      }
    } catch {
      // The page's own sentence, in the page's language, and not the server's
      // rejection code.
      message.error(t('screen.h264EngineNotChanged'));
    } finally {
      choosing.current = false;
    }
  }

  const h264Transport = h264OverQuic ? 'QUIC' : 'WebRTC';
  const entries: Entry[] = videoModes.flatMap((mode): Entry[] => {
    if (mode.key !== 'h264') return [{ key: mode.key, mode: mode.key, name: mode.name }];
    if (!offersEngine) return [{ key: mode.key, mode: mode.key, name: `H.264 (${h264Transport})` }];
    return [
      {
        key: 'h264-hardware',
        mode: 'h264',
        engine: 'hardware',
        name: `H.264 HW (${h264Transport})`
      },
      {
        key: 'h264-software',
        mode: 'h264',
        engine: 'software',
        name: `H.264 SW (${h264Transport})`
      }
    ];
  });

  // The check mark: this browser's mode, and for H.264 with engines the
  // board's choice.
  const isChecked = (entry: Entry) =>
    entry.mode === videoMode && (!entry.engine || entry.engine === board.engine);

  // The chosen engine is not what encodes: the entry says so, while this
  // browser watches H.264 and so holds the encoder to the choice.
  const encodesOtherwise = (entry: Entry) =>
    !!entry.engine &&
    isChecked(entry) &&
    board.encoder !== undefined &&
    board.encoder !== (entry.engine === 'hardware' ? 'hardware' : 'x264');

  function tooltipOf(entry: Entry): string | undefined {
    if (isUnavailable(entry.mode))
      return isWebrtcMissing
        ? t('screen.videoWebrtcUnavailable')
        : t(
            entry.mode === 'vp8'
              ? 'screen.videoVp8BlockedByFirewall'
              : quicWouldServe
                ? 'screen.videoWebrtcBlockedUseQuic'
                : 'screen.videoWebrtcBlockedByFirewall'
          );
    if (!entry.engine) return undefined;
    if (isEngineUnavailable(entry)) return t('screen.h264HardwareUnavailable');
    if (entry.engine === 'hardware' && board.available === false && board.reason)
      return t(
        mayChooseEngine ? 'screen.h264HardwareRetry' : 'screen.h264HardwareUnavailableBecause',
        { reason: board.reason }
      );
    return t(mayChooseEngine ? 'screen.h264EngineIsBoards' : 'screen.h264EngineByOperator');
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

      {entries.map((entry) => {
        if (!supportedModes.includes(entry.mode as UiVideoMode)) return null;
        if (!isDirectSupported && entry.mode === 'direct') return null;
        const unavailable = isUnavailable(entry.mode) || isEngineUnavailable(entry);
        return (
          <Tooltip
            key={entry.key}
            title={tooltipOf(entry)}
            placement="right"
            styles={{ root: { maxWidth: '270px' } }}
          >
            <div
              aria-disabled={unavailable}
              data-video-mode={entry.mode}
              data-h264-engine={entry.engine}
              data-checked={isChecked(entry) ? 'true' : 'false'}
              className={
                unavailable
                  ? 'flex cursor-not-allowed select-none items-center rounded py-1.5 pl-1 pr-5 text-neutral-500 hover:bg-neutral-700/70'
                  : 'flex cursor-pointer select-none items-center rounded py-1.5 pl-1 pr-5 hover:bg-neutral-700/70'
              }
              onClick={() => void choose(entry)}
            >
              <div className="flex h-[14px] w-[20px] items-end text-blue-500">
                {isChecked(entry) && <CheckIcon size={15} />}
              </div>
              <span data-video-mode-name={entry.engine ? entry.key : entry.mode}>{entry.name}</span>
              {encodesOtherwise(entry) && (
                <span data-h264-encodes className="whitespace-nowrap pl-2 text-xs text-amber-400">
                  {t(
                    board.encoder === 'x264'
                      ? 'screen.h264X264Encodes'
                      : 'screen.h264VideoEngineEncodes'
                  )}
                </span>
              )}
            </div>
          </Tooltip>
        );
      })}
    </>
  );

  return (
    <MenuSubmenu
      icon={<TvMinimalPlayIcon size={18} />}
      label={t('screen.video')}
      content={content}
      onOpenChange={(open) => {
        if (!open) return;
        refreshFirewallStatus();
        refreshBoardEngine();
      }}
    />
  );
};
