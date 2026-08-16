import { useEffect, useRef } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import type { SampleResponse } from "../api/runs";

export interface TrendChartProps {
  samples: SampleResponse[];
  height?: number;
}

/** uPlot wants columnar `[x[], y1[], y2[]]` data, not an array of per-tick objects. */
function toAlignedData(samples: SampleResponse[]): uPlot.AlignedData {
  // uPlot's time scale expects unix seconds, not the milliseconds `Date.getTime()` returns.
  const time = samples.map((s) => new Date(s.sample.time).getTime() / 1000);
  const pv = samples.map((s) => s.sample.pv);
  const mv = samples.map((s) => s.state.mv_value_current);
  return [time, pv, mv];
}

/**
 * A live-updating PV/MV-vs-time trend chart, backed by uPlot rather than a React charting
 * library — uPlot renders to a plain `<canvas>` and updates via its own imperative
 * `setData`, which comfortably handles samples arriving multiple times a second from
 * `useRunStream`'s SSE feed without fighting React's virtual-DOM diffing (see AGENTS.md's
 * "Chart library choice" note).
 *
 * Deliberately takes a plain `samples` array rather than an `id`/hook of its own, so the
 * exact same component renders a live-updating run (`RunDetailPage`, fed by
 * `useRunStream`) today and, later, a completed run loaded from history
 * (`history-explorer-ui`) — the two differ only in *how* the `samples` prop is produced,
 * never in how it's drawn.
 */
export function TrendChart({ samples, height = 320 }: TrendChartProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<uPlot | null>(null);

  // Creates (and tears down) the uPlot instance once per mount -- uPlot owns its own canvas
  // and redraw loop, so React's job is only to supply the container element, not to render
  // the chart's own output.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const options: uPlot.Options = {
      width: container.clientWidth || 600,
      height,
      scales: {
        x: { time: true },
        mv: {},
      },
      series: [
        {},
        { label: "PV", stroke: "#34d399", width: 2, scale: "y" },
        { label: "MV", stroke: "#38bdf8", width: 2, scale: "mv" },
      ],
      axes: [
        { stroke: "#94a3b8", grid: { stroke: "#1e293b" } },
        { stroke: "#34d399", grid: { stroke: "#1e293b" }, scale: "y" },
        { stroke: "#38bdf8", side: 1, grid: { show: false }, scale: "mv" },
      ],
      legend: { show: true },
    };

    const plot = new uPlot(options, toAlignedData(samples), container);
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
    // Only `height` from props feeds the initial `options`; `samples` is deliberately not
    // a dependency here -- the second effect below owns feeding new data into the
    // already-created instance via `setData`, so recreating the whole plot on every new
    // sample isn't needed.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [height]);

  // Feeds new samples into the already-created instance rather than recreating the plot --
  // `setData` is uPlot's own incremental-update path, and is what makes multiple updates
  // per second (live streaming) affordable.
  useEffect(() => {
    plotRef.current?.setData(toAlignedData(samples));
  }, [samples]);

  return <div ref={containerRef} />;
}
