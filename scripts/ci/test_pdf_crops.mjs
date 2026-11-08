import { strict as assert } from "node:assert";
import { test } from "node:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createRequire } from "node:module";
import { cropPage, regions } from "./pdf_crops.mjs";

const require = createRequire(resolve("ui/package.json"));
const { createCanvas, loadImage } = require("@napi-rs/canvas");

test("regions cover a page with overlap and no out-of-bounds pixels", () => {
  for (const height of [3, 101, 1000]) {
    const parts = regions(height);
    assert.equal(parts.length, 3);
    assert.equal(parts[0].top, 0);
    assert.equal(parts[2].top + parts[2].height, height);
    assert.ok(parts[0].top + parts[0].height >= parts[1].top);
    assert.ok(parts[1].top + parts[1].height >= parts[2].top);
    assert.ok(parts.every(part => part.top >= 0 && part.height > 0 && part.top + part.height <= height));
  }
  assert.throws(() => regions(0));
});

test("writes three readable cropped images to the requested temporary prefix", async () => {
  const dir = await mkdtemp(join(tmpdir(), "cordial-crops-"));
  try {
    const canvas = createCanvas(40, 100);
    const ctx = canvas.getContext("2d");
    ctx.fillStyle = "red"; ctx.fillRect(0, 0, 40, 33);
    ctx.fillStyle = "green"; ctx.fillRect(0, 33, 40, 34);
    ctx.fillStyle = "blue"; ctx.fillRect(0, 67, 40, 33);
    const input = join(dir, "page.png");
    await writeFile(input, canvas.toBuffer("image/png"));
    const outputs = await cropPage(input, join(dir, "crop"));
    assert.equal(outputs.length, 3);
    const images = await Promise.all(outputs.map(path => loadImage(path)));
    for (const image of images) {
      assert.equal(image.width, 40);
      assert.equal(image.height, 40);
    }
    const pixel = (image, y) => {
      const surface = createCanvas(40, 40);
      const context = surface.getContext("2d");
      context.drawImage(image, 0, 0);
      return [...context.getImageData(5, y, 1, 1).data];
    };
    assert.deepEqual(pixel(images[0], 0), [255, 0, 0, 255]);
    assert.deepEqual(pixel(images[1], 0), [255, 0, 0, 255]); // page row 30
    assert.deepEqual(pixel(images[1], 39), [0, 0, 255, 255]); // page row 69
    assert.deepEqual(pixel(images[2], 0), [0, 128, 0, 255]); // page row 60
    assert.deepEqual(pixel(images[2], 39), [0, 0, 255, 255]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
