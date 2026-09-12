# PDF runtime packaging

Desktop PDF pages use `pdfium-render` 0.9.4 (MIT OR Apache-2.0), compiled against the PDFium 7881 API. The default CLI build does not include this dependency. PDF extraction for the TUI retains its existing implementation.

The desktop package needs the matching native library. Prepare it explicitly:

```sh
python3 scripts/pdfium-runtime.py --platform mac-arm64 --destination dist/pdf-runtime
```

Other targets are `mac-x64`, `linux-x64` and `linux-arm64`. `--archive /path/to/pdfium-mac-arm64.tgz` uses an offline archive with the same mandatory checksum. The script pins the upstream `chromium/7881` release and verifies its SHA-256 before extracting only the library and all bundled licenses. It refuses to overwrite an existing destination. Its manifest records every packaged file's hash.

For a macOS app bundle, place `libpdfium.dylib` in `Contents/Frameworks`, and keep the entire `licenses` directory and `pdfium-runtime.json` in `Contents/Resources/pdfium`. For a Linux desktop archive, place `libpdfium.so` beside `lapis`, with the licenses and manifest in a `pdfium` directory. Include these files before signing or assembling a distributable. Do not distribute the library alone: PDFium and its bundled third-party components have their own notices; the archive's top-level MIT license covers the binary packaging project.

For development only, `LAPIS_PDFIUM_LIBRARY` can name a prepared library. Opening a document never downloads a runtime, loads a library from the current directory, or changes a system installation. A missing runtime produces an installation/retry message. Current CI archives contain the executable only; runtime assembly is still a separate packaging step.

Each page renders in a cancellable child process. Native bindings are initialized once per child; a workspace serializes workers. Only the requested page is rasterized, with at most 8 million pixels and a 20-second timeout. The active reader retains at most three pages and 32 MiB of pixel data; hidden tabs drop bitmap cache and GPU textures while retaining position. This is a cache bound, not a claim that total process RSS is 32 MiB: renderer memory, IPC buffers and GPU allocations must be measured separately.

PDFs are read-only. Previous/Next, page number, zoom and Fit width navigate pages. Page text offers selection/copy when extraction succeeds. Scanned pages remain readable as images; there is no OCR or password prompt. Reload refreshes the file and cache. Native macOS/Linux visual, input and memory acceptance is tracked separately from headless tests.

Sources: [pdfium-render](https://github.com/ajrcarey/pdfium-render), [pinned runtime release](https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium/7881), and the complete notices emitted by the packaging script.
