// The same entry form loads the unmodified historical presentation at each revision.
import { defaultWaveformStyle, drawWaveform } from "./source/waveformPresentation.ts";
import type { WaveformPage } from "./source/waveform-model.ts";

type PageInput = WaveformPage & {
  octave?: { analysis?: string | null; powers: ArrayLike<number> };
};

type View = {
  role: "detail" | "overview";
  style: Partial<typeof defaultWaveformStyle>;
  rate: number;
  width: number;
  height: number;
  first: number;
  pixelsPerSecond: number;
  frameCount: number;
};

function renderOracle(view: View, suppliedPages: PageInput[]) {
  const { width, height, rate, first, pixelsPerSecond: pps } = view;
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const offscreen = new OffscreenCanvas(width, height);
  const context = offscreen.getContext("2d")!;
  const pages = suppliedPages.map((page) => ({
    ...page,
    facts: Float32Array.from(page.facts),
    extrema: Uint32Array.from(page.extrema),
    octave: "octave" in page && page.octave ? {
      ...page.octave,
      analysis: page.octave.analysis ?? undefined,
      powers: Float64Array.from(page.octave.powers),
    } : undefined,
  }));
  const primary = pages.find((page) => page.startFrame * pps / rate <= first &&
    (page.startFrame + page.frameCount) * pps / rate > first) ?? pages[0]!;
  const style = { ...defaultWaveformStyle, ...view.style };
  const draw = drawWaveform as (...args: unknown[]) => void;
  draw(context, primary, width, height, style, {
    sampleRate: rate,
    role: view.role,
    ribbon: {
      pages,
      pixelsPerSecond: pps,
      sourceFrameCount: view.frameCount,
      framesPerRasterPixel: rate / pps,
      firstRasterPixel: first,
      firstRasterFrame: first * rate / pps,
      closeup: Math.max(0, Math.min(1, Math.log2(8 * pps / width))),
    },
  });
  const display = canvas.getContext("2d", { alpha: false })!;
  display.fillStyle = style.light ? "#f0f3f6" : "#000";
  display.fillRect(0, 0, width, height);
  display.drawImage(offscreen, 0, 0);
  return { png: canvas.toDataURL("image/png"), report: { style }, dpr: devicePixelRatio };
}

Object.assign(globalThis, { renderOracle });
