import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import clsx from 'clsx';
import { useAtom, useAtomValue } from 'jotai';
import { GripVerticalIcon, SquareIcon, XIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { getQuality } from '@/api/vm';
import {
  basicRows,
  extendedSections,
  MIN_HEIGHT,
  MIN_WIDTH,
  type QualityAnswer,
  type QualityInputs,
  type QualityLevel,
  type QualityRow,
  type VideoPath
} from '@/lib/quality-monitor.ts';
import { qualityMonitorAtom } from '@/jotai/quality-monitor.ts';
import { videoModeAtom } from '@/jotai/screen.ts';
import { useQuicVideo } from '@/hooks/useQuicVideo.ts';
import { useIsDesktopLayout } from '@/hooks/useResponsiveLayout.ts';

/** The monitor reads this often while it is open, and never while closed. */
const TICK_MS = 1000;
/** The board's answer counts as known this long. */
const FRESH_MS = 3000;
/** Room the window keeps inside the page when it is dragged. */
const KEEP_IN_VIEW_PX = 48;

/** The quality monitor's window, while it is open
 * (docs/design/h264-engine-choice-and-quality-monitor-v1.md). */
export const QualityMonitor = () => {
  const layout = useAtomValue(qualityMonitorAtom);
  return layout.open ? <QualityMonitorWindow /> : null;
};

const QualityMonitorWindow = () => {
  const { t } = useTranslation();
  const [layout, setLayout] = useAtom(qualityMonitorAtom);
  const isDesktop = useIsDesktopLayout();
  const videoMode = useAtomValue(videoModeAtom);
  const quicServes = useQuicVideo().use;
  const path: VideoPath =
    videoMode === 'mjpeg'
      ? 'mjpeg'
      : videoMode === 'direct'
        ? 'direct'
        : videoMode === 'h264' && quicServes
          ? 'quic'
          : 'webrtc';
  const codec: QualityInputs['codec'] =
    videoMode === 'mjpeg' ? 'mjpeg' : videoMode === 'vp8' ? 'vp8' : 'h264';
  const extended = layout.level === 'extended';

  const [board, setBoard] = useState<QualityAnswer | null>(null);

  useEffect(() => {
    let stopped = false;
    let known: { at: number; answer: QualityAnswer } | null = null;
    const tick = async () => {
      const answer = await getQuality()
        .then((rsp) => (rsp.code === 0 && rsp.data ? (rsp.data as QualityAnswer) : null))
        .catch(() => null);
      if (stopped) return;
      if (answer) known = { at: performance.now(), answer };
      // An answer older than three seconds is no longer known.
      setBoard(known && performance.now() - known.at <= FRESH_MS ? known.answer : null);
    };
    void tick();
    const timer = setInterval(() => void tick(), TICK_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [path]);

  // Dragged by its header, resized at its corner; kept when let go.
  const [place, setPlace] = useState({ x: layout.x, y: layout.y });
  const [size, setSize] = useState({ width: layout.width, height: layout.height });
  const gesture = useRef<{
    kind: 'move' | 'resize';
    startX: number;
    startY: number;
    from: { a: number; b: number };
  } | null>(null);

  const begin = (kind: 'move' | 'resize') => (event: ReactPointerEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    gesture.current = {
      kind,
      startX: event.clientX,
      startY: event.clientY,
      from: kind === 'move' ? { a: place.x, b: place.y } : { a: size.width, b: size.height }
    };
  };
  const move = (event: ReactPointerEvent<HTMLElement>) => {
    const current = gesture.current;
    if (!current) return;
    const dx = event.clientX - current.startX;
    const dy = event.clientY - current.startY;
    if (current.kind === 'move') {
      setPlace({
        x: Math.min(
          Math.max(0, current.from.a + dx),
          Math.max(0, window.innerWidth - KEEP_IN_VIEW_PX)
        ),
        y: Math.min(
          Math.max(0, current.from.b + dy),
          Math.max(0, window.innerHeight - KEEP_IN_VIEW_PX)
        )
      });
    } else {
      setSize({
        width: Math.min(Math.max(MIN_WIDTH, current.from.a + dx), window.innerWidth),
        height: Math.min(Math.max(MIN_HEIGHT, current.from.b + dy), window.innerHeight)
      });
    }
  };
  const end = () => {
    const current = gesture.current;
    if (!current) return;
    gesture.current = null;
    setLayout(current.kind === 'move' ? { x: place.x, y: place.y } : size);
  };

  const inputs: QualityInputs = { path, codec, board };
  const left = Math.min(place.x, Math.max(0, window.innerWidth - KEEP_IN_VIEW_PX));
  const top = Math.min(place.y, Math.max(0, window.innerHeight - KEEP_IN_VIEW_PX));

  const rowsOf = (rows: QualityRow[]) =>
    rows.map((row) => (
      <div key={row.key} data-qm-row={row.key} className="flex min-w-0 gap-2 leading-tight">
        <span className="w-[42%] shrink-0 truncate text-right text-neutral-300">{row.label}</span>
        <span data-qm-value className="min-w-0 flex-1 truncate text-white">
          {row.value}
        </span>
      </div>
    ));

  const level = (value: QualityLevel, label: string) => (
    <button
      type="button"
      data-qm-level={value}
      aria-pressed={layout.level === value}
      className={clsx(
        'rounded border-0 px-1.5 py-0.5 text-[11px]',
        layout.level === value
          ? 'bg-neutral-600 text-white'
          : 'bg-transparent text-neutral-400 hover:text-white'
      )}
      onClick={() => setLayout({ level: value })}
    >
      {label}
    </button>
  );

  // Nothing here reaches the page's own pointer, wheel or touch handlers.
  const keep = (event: { stopPropagation: () => void }) => event.stopPropagation();

  return (
    <div
      data-testid="quality-monitor"
      data-level={layout.level}
      data-opaque={layout.opaque ? 'true' : 'false'}
      className={clsx(
        'fixed z-[950] flex flex-col overflow-hidden rounded shadow-lg',
        isDesktop ? 'text-xs' : 'text-[10px]',
        layout.opaque ? 'bg-neutral-900' : 'bg-neutral-900/60 backdrop-blur-[2px]'
      )}
      style={{
        left,
        top,
        ...(extended
          ? { width: size.width, height: size.height }
          : { width: isDesktop ? 252 : 196 })
      }}
      onPointerDown={keep}
      onMouseDown={keep}
      onWheel={keep}
      onTouchStart={keep}
    >
      <div
        data-testid="quality-monitor-header"
        className="flex shrink-0 cursor-move touch-none select-none items-center gap-1 border-b border-neutral-700/60 px-1 py-0.5"
        onPointerDown={begin('move')}
        onPointerMove={move}
        onPointerUp={end}
        onPointerCancel={end}
      >
        <GripVerticalIcon size={12} className="shrink-0 text-neutral-500" />
        <span className="font-semibold text-neutral-200">QM</span>
        <div
          className="ml-auto flex items-center gap-0.5"
          onPointerDown={(event) => event.stopPropagation()}
        >
          {level('basic', t('qualityMonitor.basic'))}
          {level('extended', t('qualityMonitor.extended'))}
          {extended && (
            <button
              type="button"
              data-testid="quality-monitor-opaque"
              aria-pressed={layout.opaque}
              aria-label={t('qualityMonitor.opaque')}
              title={t('qualityMonitor.opaque')}
              className={clsx(
                'ml-0.5 rounded border-0 p-0.5',
                layout.opaque
                  ? 'bg-neutral-600 text-white'
                  : 'bg-transparent text-neutral-400 hover:text-white'
              )}
              onClick={() => setLayout({ opaque: !layout.opaque })}
            >
              <SquareIcon size={12} fill={layout.opaque ? 'currentColor' : 'none'} />
            </button>
          )}
          <button
            type="button"
            data-testid="quality-monitor-close"
            aria-label={t('qualityMonitor.close')}
            title={t('qualityMonitor.close')}
            className="ml-0.5 rounded border-0 bg-transparent p-0.5 text-neutral-400 hover:bg-neutral-700 hover:text-white"
            onClick={() => setLayout({ open: false })}
          >
            <XIcon size={12} />
          </button>
        </div>
      </div>

      {extended ? (
        <div
          data-testid="quality-monitor-body"
          className="min-h-0 flex-1 space-y-1 overflow-auto px-1.5 py-1"
        >
          {extendedSections(inputs, t).map((section) => (
            <div key={section.key} data-qm-section={section.key}>
              {section.title && (
                <div className="pt-1 text-[10px] uppercase tracking-wide text-neutral-400">
                  {section.title}
                </div>
              )}
              {rowsOf(section.rows)}
            </div>
          ))}
        </div>
      ) : (
        <div data-testid="quality-monitor-body" className="px-1.5 py-1">
          {rowsOf(basicRows(inputs, t))}
        </div>
      )}

      {extended && (
        <div
          data-testid="quality-monitor-resize"
          className="absolute bottom-0 right-0 h-3 w-3 cursor-nwse-resize touch-none border-b-2 border-r-2 border-neutral-500"
          onPointerDown={begin('resize')}
          onPointerMove={move}
          onPointerUp={end}
          onPointerCancel={end}
        />
      )}
    </div>
  );
};
