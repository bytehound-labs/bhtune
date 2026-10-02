import { useEffect, useId, useRef, useState } from "react";
import uPlot from "uplot";
// oxlint-disable-next-line import/no-unassigned-import -- uPlot's stylesheet is a required Vite side effect.
import "uplot/dist/uPlot.min.css";
import { trendXRange, type TrendPoint } from "../lib/trend";
import { useTheme } from "../useTheme";

export interface TrendChartProps {
  readonly points: readonly TrendPoint[];
  readonly tagName: string;
  readonly height?: number;
  readonly pollIntervalMs?: number | null;
}

/** uPlot wants columnar `[x[], y1[], y2[]]` data, not an array of per-tick objects. */
function toAlignedData(points: readonly TrendPoint[]): uPlot.AlignedData {
  // uPlot's time scale expects unix seconds, not the milliseconds `Date.getTime()` returns.
  const time = points.map((point) => new Date(point.time).getTime() / 1000);
  const pv = points.map((point) => point.pv);
  const mv = points.map((point) => point.mv);
  return [time, pv, mv];
}

function moveCursorToPoint(plot: uPlot, point: TrendPoint) {
  const left = plot.valToPos(Date.parse(point.time) / 1000, "x");
  if (Number.isFinite(left)) {
    plot.setCursor({ left, top: plot.rect.height / 2 }, false);
  }
}

function describeTrend(points: readonly TrendPoint[], tagName: string): string {
  const first = points[0];
  if (!first) {
    return `No PV or MV trend points are available. The recorded run tag is ${tagName}. Engineering units are not recorded.`;
  }

  const range = points.reduce(
    (current, point) => ({
      pvMinimum: Math.min(current.pvMinimum, point.pv),
      pvMaximum: Math.max(current.pvMaximum, point.pv),
      mvMinimum: Math.min(current.mvMinimum, point.mv),
      mvMaximum: Math.max(current.mvMaximum, point.mv),
    }),
    {
      pvMinimum: first.pv,
      pvMaximum: first.pv,
      mvMinimum: first.mv,
      mvMaximum: first.mv,
    },
  );
  const last = points.at(-1);
  const firstTime = new Date(first.time).toLocaleString();
  const lastTime = last ? new Date(last.time).toLocaleString() : firstTime;

  return `${points.length} plotted points from ${firstTime} to ${lastTime}. PV ranged from ${range.pvMinimum} to ${range.pvMaximum}; commanded MV ranged from ${range.mvMinimum} to ${range.mvMaximum}. Values are raw, engineering units are not recorded, and the run tag is ${tagName}. Time is on the horizontal axis, PV on the left axis, and commanded MV on the right axis.`;
}

/**
 * A live-updating PV/MV-vs-time trend chart, backed by uPlot rather than a React charting
 * library — uPlot renders to a plain `<canvas>` and updates via its own imperative
 * `setData`, which comfortably handles samples arriving multiple times a second from
 * `useRunStream`'s SSE feed without fighting React's virtual-DOM diffing (see AGENTS.md's
 * "Chart library choice" note).
 *
 * Deliberately takes plain trend points rather than an `id`/hook of its own, so the exact same
 * component renders a live-updating run (`RunDetailPage`, fed by `useRunStream`) and a
 * completed run loaded from history — the two differ only in how their points are produced,
 * never in how they're drawn.
 */
export function TrendChart({
  points,
  tagName,
  height = 320,
  pollIntervalMs,
}: TrendChartProps) {
  const descriptionId = useId();
  const selectionId = useId();
  const containerRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<uPlot | null>(null);
  const [cursorIndex, setCursorIndex] = useState<number | null>(null);
  const { theme } = useTheme();

  const selectedIndex =
    points.length === 0
      ? 0
      : Math.min(
          points.length - 1,
          Math.max(0, cursorIndex ?? points.length - 1),
        );
  const selectedPoint = points[selectedIndex];
  const selectedTime = selectedPoint
    ? new Date(selectedPoint.time).toLocaleString()
    : "No point selected";
  const selectedPointText = selectedPoint
    ? `Point ${selectedIndex + 1} of ${points.length}, ${selectedTime}. PV ${selectedPoint.pv} raw tag units; commanded MV ${selectedPoint.mv} raw tag units. Engineering units are not recorded.`
    : "No PV or MV trend point is available.";

  function selectPoint(index: number) {
    setCursorIndex(index);
    const point = points[index];
    const plot = plotRef.current;
    if (!point || !plot) return;

    moveCursorToPoint(plot, point);
  }

  // Creates (and tears down) the uPlot instance once per size/theme combination -- uPlot
  // owns its own canvas and redraw loop, so React's job is only to supply the container
  // element, not to render the chart's own output.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const styles =
      container.ownerDocument.defaultView?.getComputedStyle(container);
    const chartColor = (name: string) =>
      styles?.getPropertyValue(name).trim() ?? "";
    const pvColor = chartColor("--bhtune-chart-pv");
    const mvColor = chartColor("--bhtune-chart-mv");
    const axisColor = chartColor("--bhtune-chart-axis");
    const gridColor = chartColor("--bhtune-chart-grid");

    const options: uPlot.Options = {
      width: container.clientWidth || 600,
      height,
      scales: {
        x: {
          time: true,
          range: (_self, initMin, initMax) =>
            trendXRange(initMin, initMax, pollIntervalMs),
        },
        mv: {},
      },
      series: [
        {},
        {
          label: "PV (raw tag units)",
          stroke: pvColor,
          width: 2,
          scale: "y",
          points: { show: false },
        },
        {
          label: "MV commanded (raw tag units)",
          stroke: mvColor,
          width: 2,
          scale: "mv",
          points: { show: false },
        },
      ],
      axes: [
        { stroke: axisColor, grid: { stroke: gridColor } },
        { stroke: pvColor, grid: { stroke: gridColor }, scale: "y" },
        { stroke: mvColor, side: 1, grid: { show: false }, scale: "mv" },
      ],
      cursor: { show: true, x: true, y: false },
      legend: { show: false },
      hooks: {
        setCursor: [
          (currentPlot) => {
            const index = currentPlot.cursor.idx;
            setCursorIndex(
              typeof index === "number" && Number.isInteger(index)
                ? index
                : null,
            );
          },
        ],
      },
    };

    const plot = new uPlot(options, toAlignedData(points), container);
    plotRef.current = plot;

    const resizeObserver = new ResizeObserver(([entry]) => {
      if (entry) plot.setSize({ width: entry.contentRect.width, height });
    });
    resizeObserver.observe(container);

    return () => {
      resizeObserver.disconnect();
      plot.destroy();
      plotRef.current = null;
    };
    // Only chart configuration feeds the initial `options`; `points` is deliberately not a
    // dependency here -- the second effect below owns feeding new data into the already-created
    // instance via `setData`, so recreating the whole plot on every new point isn't needed.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [height, pollIntervalMs, theme]);

  // Feeds new points into the already-created instance rather than recreating the plot --
  // `setData` is uPlot's own incremental-update path, and is what makes multiple updates
  // per second (live streaming) affordable.
  useEffect(() => {
    plotRef.current?.setData(toAlignedData(points));
  }, [points]);

  return (
    <figure aria-describedby={descriptionId}>
      <div ref={containerRef} />
      <fieldset className="mt-3 flex min-w-0 flex-wrap gap-x-5 gap-y-2 border-0 p-0 text-xs text-slate-300">
        <legend className="sr-only">Trend series legend</legend>
        <span className="inline-flex items-center gap-2">
          <span
            aria-hidden="true"
            className="h-0.5 w-4"
            style={{ backgroundColor: "var(--bhtune-chart-pv)" }}
          />
          <span>PV (raw tag units)</span>
        </span>
        <span className="inline-flex items-center gap-2">
          <span
            aria-hidden="true"
            className="h-0.5 w-4"
            style={{ backgroundColor: "var(--bhtune-chart-mv)" }}
          />
          <span>Commanded MV (raw tag units)</span>
        </span>
      </fieldset>
      <p className="mt-1 break-words text-xs text-slate-500">
        Recorded run tag: <span className="font-mono">{tagName}</span>. PV and
        commanded MV are shown as raw values; engineering units are not
        recorded.
      </p>
      <label
        htmlFor={`${selectionId}-slider`}
        className="mt-3 block text-xs font-medium text-slate-300"
      >
        Inspect trend points
      </label>
      <input
        id={`${selectionId}-slider`}
        type="range"
        min={0}
        max={Math.max(points.length - 1, 0)}
        step={1}
        value={selectedIndex}
        disabled={points.length < 2}
        aria-valuetext={selectedPointText}
        aria-describedby={`${descriptionId} ${selectionId}-readout`}
        onChange={(event) => selectPoint(Number(event.target.value))}
        className="mt-1 w-full accent-slate-400 focus:outline-none focus-visible:ring-2 focus-visible:ring-slate-400"
      />
      <p
        id={`${selectionId}-readout`}
        className="mt-1 break-words text-xs text-slate-300"
      >
        {selectedPointText}
      </p>
      <figcaption id={descriptionId} className="mt-2 text-xs text-slate-400">
        {describeTrend(points, tagName)}
      </figcaption>
    </figure>
  );
}
