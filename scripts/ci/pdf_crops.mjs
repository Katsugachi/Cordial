// Experimental evaluation helper: split a rendered page into overlapping
// horizontal regions, without interpreting its contents. Output stays in the
// caller's temporary directory and is not part of the application bundle.
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const require = createRequire(resolve("ui/package.json"));
const { createCanvas, loadImage } = require("@napi-rs/canvas");

export function regions(height) {
  if (!Number.isInteger(height) || height < 3) throw new Error("invalid image height");
  return [[0, 0.4], [0.3, 0.7], [0.6, 1]].map(([start, end]) => {
    const top = Math.floor(height * start);
    const bottom = Math.min(height, Math.ceil(height * end));
    return { top, height: bottom - top };
  });
}

export async function cropPage(input, prefix) {
  const image = await loadImage(input);
  if (!image.width || image.width > 4000 || !image.height || image.height > 4000) throw new Error("invalid image dimensions");
  const outputs = [];
  for (const [index, region] of regions(image.height).entries()) {
    const canvas = createCanvas(image.width, region.height);
    canvas.getContext("2d").drawImage(image, 0, region.top, image.width, region.height, 0, 0, image.width, region.height);
    const path = `${prefix}-${index + 1}.png`;
    await writeFile(path, canvas.toBuffer("image/png"));
    outputs.push(path);
  }
  return outputs;
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))) {
  if (process.argv.length !== 4) throw new Error("usage: node pdf_crops.mjs IMAGE OUTPUT_PREFIX");
  console.log((await cropPage(process.argv[2], process.argv[3])).join("\n"));
}
