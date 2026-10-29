# Attribution

Every bundled font, icon, binary, and its license is listed here. The Node runtime inside the engine executable is
added in P35 (Q-P01c). Document reading (A03) adds Tesseract, Leptonica and the Tesseract model files below.

| Item | Version | License | Source |
| --- | --- | --- | --- |
| PostgreSQL server (`postgres`, `initdb`, `pg_ctl`, `share/`, extension libraries) | 16.15 | PostgreSQL License | https://www.postgresql.org, via the zonky archives below. Pinned with SHA-256 in `pg-binaries.lock`; fetched by `cargo xtask pg-fetch`, never committed |
| embedded-postgres-binaries (zonky) packaging of the PostgreSQL builds | 16.15.0 | Apache-2.0 | https://github.com/zonkyio/embedded-postgres-binaries, Maven Central `io.zonky.test.postgres` |
| IBM Plex Sans (Regular, Medium, SemiBold, Bold) and IBM Plex Mono (Regular, Medium), TTF, embedded in `cl-ui` from `assets/fonts/`; Sans Regular, SemiBold, Bold and Mono Regular also compiled into `cl-print`, which subset-embeds the glyphs used into printed PDFs and loads them as private fonts for Windows direct printing (P19) | 3.005 (Sans), 2.005 (Mono) | SIL Open Font License 1.1 (`assets/fonts/OFL.txt`, shown in About) | https://github.com/IBM/plex, `packages/plex-sans` and `packages/plex-mono`, `fonts/complete/ttf`. Copyright 2017-2018 IBM Corp., Reserved Font Name "Plex". Unmodified |
| ClientLedger application icon (`assets/icon/`) | original | MIT (ClientLedger code license) | Drawn for this project (Q-122); no third-party artwork |
| Libraries inside the zonky archives: OpenSSL, ICU, zlib, libxml2, libxslt, liblz4, libzstd, libcurl, libwinpthread, GNU libintl and libiconv (Windows) | as built by zonky | Each library's own license (OpenSSL: Apache-2.0 or the OpenSSL license; ICU: Unicode/ICU; zlib; MIT; BSD-2/3; curl; libintl and libiconv: LGPL) | Same archives. License texts must ship with the installer (P35); see Q-P03e |

## PDFium (document previews, P17)

The PDF preview loads the PDFium shared library at run time (`pdfium.dll`, `libpdfium.dylib` or `libpdfium.so`).
It is not committed: `cargo xtask pdfium-fetch` downloads the archive pinned with SHA-256 in `pdfium-binaries.lock`
(hashes checked against the digests GitHub publishes for the release assets) and unpacks only the library,
`LICENSE` and `licenses/*`. The license texts ship beside the library: `target/pdfium/<triple>/` in development,
`pdfium/` next to the app in preview builds (`cargo xtask preview`) and in the installers (P35). The build has no
V8 (JavaScript) and no XFA (`args.gn`: `pdf_enable_v8 = false`, `pdf_enable_xfa = false`). Every third-party
license below was read from the archive's `licenses/` folder for all four platforms (the texts are the same on
each); none is copyleft. Owner licensing review: P39, with Q-P03e.

| Item | Version | License | Source |
| --- | --- | --- | --- |
| PDFium shared library | chromium/7881 (151.0.7881.0) | BSD-3-Clause (Copyright 2014 The PDFium Authors); some parts Apache-2.0 (`licenses/pdfium.txt` carries both texts) | https://pdfium.googlesource.com/pdfium, built by the project below |
| pdfium-binaries packaging | chromium/7881 | MIT (`LICENSE`, Copyright 2014-2025 Benoit Blanchon) | https://github.com/bblanchon/pdfium-binaries, release `chromium/7881`, archives `pdfium-{win-x64,mac-arm64,mac-x64,linux-x64}.tgz` |
| FreeType (inside PDFium) | as built in chromium/7881 | FreeType License (FTL). FreeType is offered under the FTL or GPLv2; ClientLedger elects the **FTL** (`licenses/freetype.txt`) | https://freetype.org |
| libjpeg-turbo (inside PDFium) | as built in chromium/7881 | IJG License and BSD-3-Clause; SIMD parts zlib License (`licenses/libjpeg_turbo.md`, `licenses/libjpeg_turbo.ijg`) | https://libjpeg-turbo.org |
| libpng (inside PDFium) | as built in chromium/7881 | PNG Reference Library License version 2 (`licenses/libpng.txt`) | http://www.libpng.org |
| zlib (inside PDFium) | 1.3.2.1 (Chromium's copy) | zlib License (`licenses/zlib.txt`) | https://zlib.net |
| OpenJPEG (inside PDFium) | as built in chromium/7881 | BSD-2-Clause (`licenses/libopenjpeg.txt`) | https://www.openjpeg.org |
| Little CMS (lcms2, inside PDFium) | as built in chromium/7881 | MIT (`licenses/lcms.txt`) | https://www.littlecms.com |
| LibTIFF (inside PDFium) | as built in chromium/7881 | libtiff License, BSD-style (`licenses/libtiff.txt`) | https://libtiff.gitlab.io/libtiff/ |
| Abseil (inside PDFium) | as built in chromium/7881 | Apache-2.0 (`licenses/abseil.txt`) | https://abseil.io |
| ICU (inside PDFium) | as built in chromium/7881 | Unicode License v3 (`licenses/icu.txt`). The file also lists ICU's own third-party notices; the GPL-licensed entries there (`aclocal.m4`, `config.guess`, `install-sh`) are ICU4C build scripts under the Autoconf exception and are not part of the library | https://icu.unicode.org |
| Anti-Grain Geometry 2.3 (inside PDFium) | 2.3 | AGG 2.3 permissive notice (`licenses/agg23.txt`) | http://www.antigrain.com |
| fast_float (inside PDFium) | as built in chromium/7881 | MIT (`licenses/fast_float.txt`) | https://github.com/fastfloat/fast_float |
| simdutf (inside PDFium) | as built in chromium/7881 | MIT (`licenses/simdutf.txt`) | https://github.com/simdutf/simdutf |
| LLVM libc (inside PDFium) | as built in chromium/7881 | Apache-2.0 WITH LLVM-exception (`licenses/llvm-libc.txt`) | https://libc.llvm.org |

Notices the licenses ask for in the documentation:

- Portions of this software are copyright © 2026 The FreeType Project (https://freetype.org). All rights reserved.
- This software is based in part on the work of the Independent JPEG Group.
- PDFium, Copyright 2014 The PDFium Authors.

## pdf.js inside the engine executable (reading PDF bills, P17)

The engine reads the text layer of a PDF (a vendor's bill, a bank statement) with pdf.js. `cargo xtask engine`
bundles pdf.js and its worker (`legacy/build/pdf.mjs` and `legacy/build/pdf.worker.mjs`) into the engine
executable's JavaScript, from the version the engine's `package-lock.json` pins. The worker runs inside the engine
process; nothing is downloaded. pdf.js's fonts (`standard_fonts/`) and character maps (`cmaps/`) are not shipped:
reading text does not need them for the Latin-script PDFs the app reads. The Apache-2.0 license text must ship
with the installer (P35).

| Item | Version | License | Source |
| --- | --- | --- | --- |
| pdf.js (`pdfjs-dist`, legacy build: `pdf.mjs`, `pdf.worker.mjs`) | 4.10.38 | Apache-2.0 (Copyright 2024 Mozilla Foundation; `node_modules/pdfjs-dist/LICENSE`) | https://github.com/mozilla/pdf.js, npm `pdfjs-dist` |
| core-js polyfills (inside the pdf.js legacy build) | as bundled in pdfjs-dist 4.10.38 | MIT (© 2014-2024 Denis Pushkarev) | https://github.com/zloirock/core-js |

**Not shipped:** poppler (`pdftoppm`, `pdftotext`, `pdfimages`) is GPL and is not part of ClientLedger. The engine
uses `pdftotext` only as a fallback after pdf.js, and `pdftoppm` only to turn a scanned PDF (no text layer) into
page pictures. In ClientLedger neither is needed: since A03 the desktop reads text PDFs, scans and photos itself
(PDFium and Tesseract below, in a separate reading program). P17's engine-side Read with AI, offered only on engines
without document reading assist, still says "reading scanned PDFs isn't available yet" for a scan. If an owner has
installed poppler themselves, the engine may find it on their PATH; ClientLedger never installs or bundles it.

## Tesseract OCR (reading scans and photos, A03)

The `clientledger-docread` reading program sends each scanned page to the `tesseract` program. Nothing here is
committed: `cargo xtask ocr-build` downloads the sources pinned with SHA-256 in `ocr.lock` and builds them with every
optional dependency off (no image codecs, zlib, curl, libarchive, OpenMP, training tools or graphics), and
`cargo xtask ocr-models` downloads the model files pinned there. The output is `target/ocr/<triple>/{tesseract[.exe],
tessdata/, LICENSES/}` in development and `ocr/` next to the app in preview builds and installers (P35). `LICENSES/`
holds `tesseract-LICENSE.txt`, `leptonica-LICENSE.txt`, `tessdata-LICENSE.txt` and `tessdata_best-LICENSE.txt`,
copied from the pinned sources. Models are never downloaded at run time. Licenses were read from the pinned sources
on 2026-10-26.

| Item | Version | License | Source |
| --- | --- | --- | --- |
| Tesseract (`tesseract` program; the legacy engine is kept for orientation detection) | 5.5.3 (tag commit 6951ffe1) | Apache-2.0 (`LICENSE`). The 5.5.3 tag has **no NOTICE file** (checked in the tag's tree and the extracted archive), so Apache-2.0 §4(d) adds nothing to carry; the source headers are Apache-2.0 throughout | https://github.com/tesseract-ocr/tesseract, GitHub tag archive `5.5.3` |
| Leptonica (image library, linked statically into `tesseract`) | 1.87.0 | BSD-2-Clause style (`leptonica-license.txt`, Copyright (C) 2001-2020 Leptonica). `rbtree.c`/`rbtree.h` are adapted from code placed in the public domain (CC0) | https://github.com/DanBloomberg/leptonica, release asset `leptonica-1.87.0.tar.gz` |
| `tessdata_fast` `eng.traineddata`, `osd.traineddata` | commit 87416418 | Apache-2.0 (`LICENSE`; no NOTICE file in the repository at that commit) | https://github.com/tesseract-ocr/tessdata_fast |
| `tessdata_best` `eng.traineddata`, installed as `engbest.traineddata` (second read of low-confidence pages, Q-A03b) | commit e12c65a9 | Apache-2.0 (`LICENSE`; no NOTICE file in the repository at that commit) | https://github.com/tesseract-ocr/tessdata_best |
| GNU libstdc++ and libgcc (Linux `tesseract` only, linked statically with `-static-libstdc++ -static-libgcc`) | as shipped by the build image's GCC (ubuntu-22.04) | GPL-3.0 with the **GCC Runtime Library Exception 3.1**: the exception allows a program compiled with GCC to carry these runtime parts under its own terms, so `tesseract` stays Apache-2.0 | https://gcc.gnu.org/onlinedocs/libstdc++/manual/license.html |

On Windows the program is built with the static MSVC runtime (`MultiThreaded`) and imports only KERNEL32 and
ADVAPI32; on macOS it links only the system's `libSystem` and `libc++`. A linkage check in `ocr-build` refuses any
other library.

Notice the Leptonica license asks for in the documentation:

- Leptonica, Copyright (C) 2001-2020 Leptonica. All rights reserved. Redistributed under the BSD-2-Clause style
  license in `ocr/LICENSES/leptonica-LICENSE.txt`.

## Test fonts (never shipped, Q-A03f)

`cargo xtask gen-invoices` makes the synthetic bills and receipts the reading tests use. Besides IBM Plex (above), it
uses three fonts from the google/fonts repository at commit `e7db6ca6`, pinned by SHA-256 in `docread-fonts.lock` and
downloaded with their `OFL.txt` into `target/docread-fonts/`. They are **never committed, bundled or shipped**; the
committed samples in `fixtures/docs/read/` use IBM Plex only.

| Item | License | Source |
| --- | --- | --- |
| Courier Prime (Regular, Bold) | SIL Open Font License 1.1 | https://github.com/google/fonts, `ofl/courierprime` |
| Kalam (Regular) | SIL Open Font License 1.1 | https://github.com/google/fonts, `ofl/kalam` |
| PT Serif (Regular, Bold; the held-out font) | SIL Open Font License 1.1 | https://github.com/google/fonts, `ofl/ptserif` |

## Rust crates added for document reading (A03)

All are MIT or Apache-2.0 as `cargo xtask licenses` and `cargo deny check` require; listed because they are new
edges, read from each crate's `Cargo.toml` and license files in the registry.

| Crate | Version | License | Used by |
| --- | --- | --- | --- |
| `unicode-normalization` | 0.1.25 | MIT OR Apache-2.0 | `cl-docread` (vendor names, NFKC) |
| `nix` (`resource`, `sched`, `process`, `signal`) | 0.31.3 | MIT | the reading program's limits; `cl-host` (Unix) |
| `sysinfo` (`system`) | 0.39.6 | MIT | `cl-host` memory watchdog |
| `windows-sys` (`JobObjects`) | 0.61.2 | MIT OR Apache-2.0 | `cl-host`'s one job-limits call (Q-A03s) |
| `pdf-writer` | 0.15.0 | MIT OR Apache-2.0 | xtask only (`gen-invoices`), not shipped |
| `subsetter` (no default features) | 0.2.6 | MIT OR Apache-2.0 | xtask only, not shipped |
| `skrifa` (with `read-fonts` 0.41.0, `font-types` 0.12.7) | 0.44.0 | MIT OR Apache-2.0 | xtask only (font metrics; replaces the unmaintained `ttf-parser`, RUSTSEC-2026-0192), not shipped |

`pdfium-render`, `image` and `tiff` were already used by the viewer (P17, A02); the reading program uses the same
versions and features.

## PostgreSQL notice

PostgreSQL Database Management System, portions Copyright (c) 1996-2026 The
PostgreSQL Global Development Group, portions Copyright (c) 1994 The Regents of
the University of California. The full PostgreSQL License text is at
https://www.postgresql.org/about/licence/ and ships with the installer (P35).
