import { useEffect, useRef } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { trendXRange, type TrendPoint } from "../lib/trend";
import { useTheme } from "../useTheme";

export interface TrendChartProps {
  points: TrendPoint[];
  height?: number;
  pollIntervalMs?: number | null;
}

/** uPlot wants columnar `[x[], y1[], y2[]]` data, not an array of per-tick objects. */
function toAlignedData(points: readonly TrendPoint[]): uPlot.AlignedData {
  // uPlot's time scale expects unix seconds, not the milliseconds `Date.getTime()` returns.
  const time = points.map((point) => new Date(point.time).getTime() / 1000);
  const pv = points.map((point) => point.pv);
  const mv = points.map((point) => point.mv);
  return [time, pv, mv];
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
  height = 320,
  pollIntervalMs,
}: TrendChartProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<uPlot | null>(null);
  const { theme } = useTheme();

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
        { label: "PV", stroke: pvColor, width: 2, scale: "y" },
        { label: "MV", stroke: mvColor, width: 2, scale: "mv" },
      ],
      axes: [
        { stroke: axisColor, grid: { stroke: gridColor } },
        { stroke: pvColor, grid: { stroke: gridColor }, scale: "y" },
        { stroke: mvColor, side: 1, grid: { show: false }, scale: "mv" },
      ],
      legend: { show: true },
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

  return <div ref={containerRef} />;
}
