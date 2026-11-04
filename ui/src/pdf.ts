// Local PDF attachment preparation. PDF.js renders in a worker; no document
// scripts are enabled and the complete original bytes are never sent to a
// network service. Limits are deliberately explicit rather than silently
// dropping pages or shrinking an entire long worksheet into one image.

export const MAX_PDF_BYTES = 10_000_000;
export const MAX_PDF_PAGES = 3;
export const MAX_PAGE_PIXELS = 2_000_000;
// The core folds at least 4,000 characters per text attachment into context.
// Reject longer pages rather than silently dropping worksheet questions.
export const MAX_PAGE_TEXT_CHARS = 4_000;
export const MAX_EXTRACTED_CHARS = 12_000;

export interface PdfPage {
  text: string;
  image: string; // PNG data URL
}

/** Parse and render each page of a small, user-selected PDF. */
export async function preparePdf(bytes: Uint8Array, options: { workerSrc?: string; createCanvas?: () => HTMLCanvasElement } = {}): Promise<PdfPage[]> {
  if (bytes.length > MAX_PDF_BYTES || bytes.length < 5) throw new Error("PDF must be at most 10 MB and nonempty.");
  if (String.fromCharCode(...bytes.slice(0, 5)) !== "%PDF-") throw new Error("The selected file is not a PDF.");

  const pdfjs = await import("pdfjs-dist/legacy/build/pdf.mjs");
  const workerSrc = options.workerSrc ?? (await import("pdfjs-dist/legacy/build/pdf.worker.mjs?url")).default;
  pdfjs.GlobalWorkerOptions.workerSrc = workerSrc;
  const loading = pdfjs.getDocument({ data: bytes, enableXfa: false, stopAtErrors: true, useSystemFonts: true });
  let pdf: Awaited<typeof loading.promise> | undefined;
  try {
    pdf = await loading.promise;
    if (pdf.numPages < 1 || pdf.numPages > MAX_PDF_PAGES) {
      throw new Error(`PDF has ${pdf.numPages} pages; the current limit is ${MAX_PDF_PAGES}. No pages were attached.`);
    }
    let totalText = 0;
    const pages: PdfPage[] = [];
    for (let index = 1; index <= pdf.numPages; index++) {
      const page = await pdf.getPage(index);
      try {
        const content = await page.getTextContent();
        let text = "";
        for (const item of content.items) {
          if ("str" in item) {
            const part = item.str + (item.hasEOL ? "\n" : " ");
            totalText += part.length;
            if (totalText > MAX_EXTRACTED_CHARS || text.length + part.length > MAX_PAGE_TEXT_CHARS) {
              throw new Error("The PDF text exceeds the per-page read limit; no pages were attached.");
            }
            text += part;
          }
        }
        const base = page.getViewport({ scale: 1 });
        if (!Number.isFinite(base.width) || !Number.isFinite(base.height) || base.width <= 0 || base.height <= 0 || base.width > 10_000 || base.height > 10_000) {
          throw new Error("PDF page dimensions are invalid or too large.");
        }
        const scale = Math.min(2, Math.sqrt(MAX_PAGE_PIXELS / (base.width * base.height)));
        const viewport = page.getViewport({ scale });
        const canvas = options.createCanvas ? options.createCanvas() : document.createElement("canvas");
        canvas.width = Math.max(1, Math.floor(viewport.width));
        canvas.height = Math.max(1, Math.floor(viewport.height));
        const context = canvas.getContext("2d");
        if (!context) throw new Error("The PDF renderer could not create a canvas.");
        await page.render({ canvas, canvasContext: context, viewport }).promise;
        const image = canvas.toDataURL("image/png");
        canvas.width = 0;
        canvas.height = 0;
        pages.push({ text, image });
      } finally {
        page.cleanup();
      }
    }
    return pages;
  } finally {
    await loading.destroy();
  }
}
