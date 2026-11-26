import { app, BrowserWindow } from "electron";
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

type Case = { id: string; pages: string; dpr: number };
type Run = { cases: Case[] };
type Frame = { png: string; dpr: number; report: unknown } | { error: string };

const root = resolve(process.argv[2] ?? ".");
app.setPath("userData", join(root, "electron-profile"));
app.commandLine.appendSwitch("use-angle", "swiftshader");
app.commandLine.appendSwitch("enable-unsafe-swiftshader");
app.commandLine.appendSwitch("force-color-profile", "srgb");

void app.whenReady().then(async () => {
  const window = new BrowserWindow({
    show: false,
    paintWhenInitiallyHidden: false,
    webPreferences: { offscreen: true, backgroundThrottling: false },
  });
  await window.loadFile(join(root, "oracle.html"));
  const displayScale: number = await window.webContents.executeJavaScript("devicePixelRatio");
  const { cases }: Run = JSON.parse(await readFile(join(root, "render.json"), "utf8"));
  for (const item of cases) {
    window.webContents.setZoomFactor(item.dpr / displayScale);
    await window.webContents.executeJavaScript(`new Promise((resolve, reject) => {
      const limit = performance.now() + 3000;
      function ready() {
        if (Math.abs(devicePixelRatio - ${item.dpr}) <= 0.01) resolve();
        else if (performance.now() >= limit) reject(new Error("Backing scale did not settle"));
        else setTimeout(ready, 10);
      }
      ready();
    })`);
    const pages = await readFile(join(root, item.pages), "utf8");
    const result: Frame = await window.webContents.executeJavaScript(
      `(() => {
        try { return globalThis.renderOracle(${JSON.stringify(item)}, ${pages}); }
        catch (error) { return { error: error.stack ?? String(error) }; }
      })()`,
    );
    if ("error" in result) throw new Error(`${item.id}: ${result.error}`);
    if (Math.abs(result.dpr - item.dpr) > 0.01)
      throw new Error(`Unexpected DPR ${result.dpr} for ${item.id}, expected ${item.dpr}`);
    await writeFile(join(root, `${item.id}.png`), Buffer.from(result.png.split(",")[1]!, "base64"));
    await writeFile(join(root, `${item.id}.json`), JSON.stringify(result.report));
  }
  await writeFile(join(root, "electron.json"), JSON.stringify(process.versions, null, 2));
  window.destroy();
  app.exit(0);
}).catch(async (error: unknown) => {
  await writeFile(join(root, "electron-error.txt"), String(error instanceof Error ? error.stack : error));
  app.exit(1);
});
