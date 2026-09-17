// Rasterises the SVG masters written by generate.py. Kept in JS because resvg
// is the only renderer available here without system packages; generate.py
// invokes this and then packs the PNGs into .ico/.icns.
import { Resvg } from "@resvg/resvg-js";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const work = join(here, "build");
const png = join(work, "png");
mkdirSync(png, { recursive: true });

const jobs = [
  ["logo.svg", "logo-512.png", 512],
  ["logo.svg", "logo-1024.png", 1024],
  ["favicon.svg", "favicon-16.png", 16],
  ["favicon.svg", "favicon-32.png", 32],
  ["favicon.svg", "favicon-48.png", 48],
  ["favicon.svg", "favicon-64.png", 64],
  ["favicon.svg", "favicon-128.png", 128],
  ["favicon.svg", "favicon-256.png", 256],
  ["apple-touch-icon.svg", "ios-180.png", 180],
  ["apple-touch-icon.svg", "ios-512.png", 512],
];

// Tauri bundle sizes plus the Windows Store logo set.
for (const s of [30, 32, 44, 50, 64, 71, 89, 107, 128, 142, 150, 256, 284, 310, 512, 1024]) {
  jobs.push(["logo.svg", `tile-${s}.png`, s]);
}

for (const [src, dst, width] of jobs) {
  const svg = readFileSync(join(work, src), "utf8");
  const r = new Resvg(svg, { fitTo: { mode: "width", value: width } });
  writeFileSync(join(png, dst), r.render().asPng());
}
console.log(`rendered ${jobs.length} png files -> ${png}`);
