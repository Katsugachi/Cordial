// @vitest-environment node
import { describe, expect, it } from "vitest";
import { createCanvas, loadImage } from "@napi-rs/canvas";
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { MAX_PDF_BYTES, preparePdf } from "../src/pdf";

const require = createRequire(import.meta.url);
const workerSrc = pathToFileURL(require.resolve("pdfjs-dist/legacy/build/pdf.worker.mjs")).href;
const options = { workerSrc, createCanvas: () => createCanvas(1, 1) as unknown as HTMLCanvasElement };

function fixture(pageCount: number): Uint8Array {
  const font = 3 + pageCount * 2;
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    `<< /Type /Pages /Count ${pageCount} /Kids [${Array.from({ length: pageCount }, (_, i) => `${3 + i * 2} 0 R`).join(" ")}] >>`,
  ];
  for (let i = 0; i < pageCount; i++) {
    const page = 3 + i * 2;
    const contents = "BT /F1 20 Tf 20 160 Td (Question 1) Tj ET\n0 0 0 rg 20 20 50 50 re f";
    objects.push(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 ${font} 0 R >> >> /Contents ${page + 1} 0 R >>`);
    objects.push(`<< /Length ${contents.length} >>\nstream\n${contents}\nendstream`);
  }
  objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
  let pdf = "%PDF-1.4\n";
  const offsets = [0];
  objects.forEach((object, i) => {
    offsets.push(Buffer.byteLength(pdf));
    pdf += `${i + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xref = Buffer.byteLength(pdf);
  pdf += `xref\n0 ${offsets.length}\n0000000000 65535 f \n`;
  pdf += offsets.slice(1).map((n) => `${String(n).padStart(10, "0")} 00000 n \n`).join("");
  pdf += `trailer\n<< /Size ${offsets.length} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new Uint8Array(Buffer.from(pdf));
}

describe("PDF attachment preparation", () => {
  it("renders page graphics and extracts text", async () => {
    const pages = await preparePdf(fixture(1), options);
    expect(pages).toHaveLength(1);
    expect(pages[0].text).toContain("Question 1");
    const image = await loadImage(pages[0].image);
    expect(image.width).toBeGreaterThan(0);
    expect(image.height).toBeGreaterThan(0);
    const canvas = createCanvas(image.width, image.height);
    const ctx = canvas.getContext("2d");
    ctx.drawImage(image, 0, 0);
    expect(ctx.getImageData(60, 300, 1, 1).data[0]).toBeLessThan(10);
  });

  it("rejects invalid, oversized and overlong documents without partial output", async () => {
    await expect(preparePdf(new Uint8Array([1, 2, 3]))).rejects.toThrow();
    await expect(preparePdf(new Uint8Array(MAX_PDF_BYTES + 1))).rejects.toThrow();
    await expect(preparePdf(fixture(4), options)).rejects.toThrow(/No pages were attached/);
  });

  it("can render an operator-supplied PDF locally without committing it", async () => {
    if (!process.env.CORDIAL_PDF_TEST_PATH) return;
    const bytes = new Uint8Array(readFileSync(process.env.CORDIAL_PDF_TEST_PATH));
    const pages = await preparePdf(bytes, options);
    expect(pages).toHaveLength(1);
    expect(pages[0].text).toContain("Question 1");
    expect(pages[0].text).toContain("Question 2");
    expect(pages[0].text).toContain("Question 3");
    if (process.env.CORDIAL_PDF_RENDER_PATH) {
      writeFileSync(process.env.CORDIAL_PDF_RENDER_PATH, Buffer.from(pages[0].image.split(",")[1], "base64"));
    }
    if (process.env.CORDIAL_PDF_TEXT_PATH) writeFileSync(process.env.CORDIAL_PDF_TEXT_PATH, pages[0].text);
  });
});
