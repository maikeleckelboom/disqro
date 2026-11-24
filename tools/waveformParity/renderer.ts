// Bundled beside the unmodified, pinned Dekzer renderer modules.
import { StrokeColourField, StrokeSurface, strokeColumns } from "./source/waveformDirect.ts";
import { closeupWeight } from "./source/waveformGeometry.ts";
import {
  drawThreeBandColumns,
  drawWaveform,
  projectSourceColumns,
} from "./source/waveformPresentation.ts";
import type { WaveformStyle } from "./source/waveformPresentation.ts";
import { cellCores, SourceLeaves } from "./source/waveformSourceCells.ts";
import { STROKE_POLICY, cellOfPixel, cellStartFrame, strokeBlockLaw } from "./source/waveformStroke.ts";
import type { WaveformPage } from "./source/waveform-model.ts";

type View = {
  role: "detail" | "overview";
  style: WaveformStyle;
  rate: number;
  first: number;
  pixelsPerSecond: number;
  width: number;
  height: number;
  origin: number;
  frameCount: number;
  columns?: number[];
};

function typedPage(page: WaveformPage): WaveformPage {
  return {
    ...page,
    facts: Float32Array.from(page.facts),
    extrema: Uint32Array.from(page.extrema),
    spectrum: page.spectrum && Float64Array.from(page.spectrum),
    octave: page.octave && { ...page.octave, powers: Float64Array.from(page.octave.powers) },
    sourceDetail: page.sourceDetail && {
      ...page.sourceDetail,
      bounds: Float32Array.from(page.sourceDetail.bounds),
      offsets: Uint16Array.from(page.sourceDetail.offsets),
    },
  };
}

function renderOracle(view: View, suppliedPages: WaveformPage[]) {
  const { rate, width, height, first, pixelsPerSecond: pps, origin, frameCount, style } = view;
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  canvas.style.width = `${width / devicePixelRatio}px`;
  canvas.style.height = `${height / devicePixelRatio}px`;
  const pages = suppliedPages.map(typedPage);
  let report: unknown = {};
  if (style.mode === "stroke" && view.role === "detail" && strokeBlockLaw(rate, pps)) {
    const leaves = new SourceLeaves(pages, frameCount, origin);
    const color = new StrokeColourField();
    color.reset(pages, frameCount);
    const firstCell = cellOfPixel(first, pps, rate) - 1;
    const count = cellOfPixel(first + width - 1, pps, rate) + 2 - firstCell;
    const starts = Float64Array.from({ length: count + 1 }, (_, i) =>
      cellStartFrame(firstCell + i, rate, pps));
    const min = new Float64Array(count);
    const max = new Float64Array(count);
    const held = new Uint8Array(count);
    cellCores(leaves, starts, frameCount, min, max, held);
    const columns = view.columns
      ? { first, count: width, data: Float32Array.from(view.columns), complete: true }
      : strokeColumns(leaves, color, rate, pps, first, width, height, style, frameCount);
    const surface = new StrokeSurface(canvas);
    surface.upload(columns);
    surface.draw(
      width, height, 0, style.light ? 0 : STROKE_POLICY.neutral, closeupWeight(width / pps),
      style.light ? [240 / 255, 243 / 255, 246 / 255] : [0, 0, 0],
    );
    report = {
      firstCell, starts: Array.from(starts), min: Array.from(min), max: Array.from(max),
      held: Array.from(held), columns: Array.from(columns.data), complete: columns.complete,
    };
  } else {
    const offscreen = new OffscreenCanvas(width, height);
    const context = offscreen.getContext("2d")!;
    if (view.role === "overview") {
      drawWaveform(context, pages[0]!, width, height, style, { sampleRate: rate, role: "overview" });
    } else if (style.mode === "threeBand") {
      const columns = projectSourceColumns(pages, Math.floor(first * rate / pps),
        Math.ceil((first + width) * rate / pps), (frame) => frame * pps / rate - first);
      drawThreeBandColumns(context, columns, height, style, rate / pps, rate);
      report = { columns };
    } else {
      drawWaveform(context, pages[0]!, width, height, style, {
        sampleRate: rate,
        role: "detail",
        geometry: {
          pages, pixelsPerSecond: pps, sourceFrameCount: frameCount,
          firstRasterPixel: first, framesPerRasterPixel: rate / pps,
          closeup: closeupWeight(width / pps),
        },
      });
    }
    const display = canvas.getContext("2d", { alpha: false })!;
    display.fillStyle = style.light ? "#f0f3f6" : "#000";
    display.fillRect(0, 0, width, height);
    display.drawImage(offscreen, 0, 0);
  }
  const png = canvas.toDataURL("image/png");
  // Free the temporary comparison window's WebGL context before the next case.
  canvas.getContext("webgl2")?.getExtension("WEBGL_lose_context")?.loseContext();
  return { png, report, dpr: devicePixelRatio };
}

Object.assign(globalThis, { renderOracle });
