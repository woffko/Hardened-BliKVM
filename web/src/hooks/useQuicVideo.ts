import { useEffect, useState } from 'react';
import { useAtom, useAtomValue } from 'jotai';

import { supportsBoardFeature, supportsVideoTransport } from '@/lib/capabilities.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { videoModeAtom } from '@/jotai/screen.ts';
import {
  quicStatusAtom,
  quicVideoAtom,
  quicVideoGivenUpAtom,
  transportChoiceAtom
} from '@/jotai/transport.ts';

/** The codec the board's H.264 encoder makes: constrained baseline. */
const H264_CODEC = 'avc1.42e028';

/**
 * Whether this browser decodes H.264 with WebCodecs: asked once, and known
 * at once where there is no WebCodecs at all.
 */
let decoderKnown: boolean | null = typeof VideoDecoder === 'undefined' ? false : null;
let decoderSupport: Promise<boolean> | null = null;
function h264DecoderIsSupported(): Promise<boolean> {
  decoderSupport ??= (
    decoderKnown === false
      ? Promise.resolve(false)
      : VideoDecoder.isConfigSupported({ codec: H264_CODEC, optimizeForLatency: true }).then(
          (support) => support.supported === true,
          () => false
        )
  ).then((supported) => {
    decoderKnown = supported;
    return supported;
  });
  return decoderSupport;
}

export type QuicVideoUse = {
  /** The Screen menu's H.264 mode is served over the QUIC session now. */
  use: boolean;
  /**
   * H.264 is, or would be if it were the Screen menu's mode, served over the
   * QUIC session: the menu offers the mode so, also where WebRTC is not to
   * be had (a firewall mode that closes its ports, a browser without it).
   */
  offered: boolean;
  /**
   * Why H.264 stays on WebRTC although this browser uses QUIC: `decoder`
   * (no H.264 decoder in WebCodecs), or the gateway's or the session's
   * reason. `null` when it does not stay there, or QUIC is not chosen.
   */
  kept: string | null;
};

/**
 * Whether the H.264 picture comes over the QUIC session: when this browser
 * uses QUIC, the board offers it, the browser can decode H.264 with
 * WebCodecs, and neither the session nor the picture has failed on this page
 * load. Everything else leaves H.264 on WebRTC.
 */
export function useQuicVideo(): QuicVideoUse {
  const capabilities = useAtomValue(capabilitiesAtom);
  const choice = useAtomValue(transportChoiceAtom);
  const status = useAtomValue(quicStatusAtom);
  const video = useAtomValue(quicVideoAtom);
  const videoMode = useAtomValue(videoModeAtom);
  const [givenUp, setGivenUp] = useAtom(quicVideoGivenUpAtom);
  const [decoder, setDecoder] = useState<boolean | null>(decoderKnown);

  useEffect(() => {
    if (decoderKnown !== null) return;
    let cancelled = false;
    void h264DecoderIsSupported().then((supported) => {
      if (!cancelled) setDecoder(supported);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const able =
    choice === 'quic' &&
    supportsBoardFeature(capabilities, 'network_quic') &&
    supportsVideoTransport(capabilities, 'web_rtc_janus_h264');
  const wanted = able && videoMode === 'h264';

  // The gateway has no picture for this session, or the session failed:
  // WebRTC serves for the rest of this page load.
  const failure =
    video.state === 'unavailable' ? video.reason : status.state === 'failed' ? status.reason : null;
  useEffect(() => {
    if (wanted && failure && !givenUp) setGivenUp(failure);
  }, [failure, givenUp, setGivenUp, wanted]);

  if (!able) return { use: false, offered: false, kept: null };
  if (decoder === false) return { use: false, offered: false, kept: wanted ? 'decoder' : null };
  const stopped = givenUp ?? failure;
  if (stopped) return { use: false, offered: false, kept: wanted ? stopped : null };
  // While the decoder is being asked about, and until the session has
  // failed, the page waits for QUIC rather than start WebRTC and switch: the
  // session is started as soon as the page is there.
  return { use: wanted, offered: true, kept: null };
}
