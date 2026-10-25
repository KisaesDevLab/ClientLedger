# Attribution

Every bundled font, icon, binary, and its license is listed here. The Node runtime inside the engine executable is
added in P35 (Q-P01c).

| Item | Version | License | Source |
| --- | --- | --- | --- |
| PostgreSQL server (`postgres`, `initdb`, `pg_ctl`, `share/`, extension libraries) | 16.15 | PostgreSQL License | https://www.postgresql.org, via the zonky archives below. Pinned with SHA-256 in `pg-binaries.lock`; fetched by `cargo xtask pg-fetch`, never committed |
| embedded-postgres-binaries (zonky) packaging of the PostgreSQL builds | 16.15.0 | Apache-2.0 | https://github.com/zonkyio/embedded-postgres-binaries, Maven Central `io.zonky.test.postgres` |
| IBM Plex Sans (Regular, Medium, SemiBold, Bold) and IBM Plex Mono (Regular, Medium), TTF, embedded in `cl-ui` from `assets/fonts/` | 3.005 (Sans), 2.005 (Mono) | SIL Open Font License 1.1 (`assets/fonts/OFL.txt`, shown in About) | https://github.com/IBM/plex, `packages/plex-sans` and `packages/plex-mono`, `fonts/complete/ttf`. Copyright 2017-2018 IBM Corp., Reserved Font Name "Plex". Unmodified |
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

## PostgreSQL notice

PostgreSQL Database Management System, portions Copyright (c) 1996-2026 The
PostgreSQL Global Development Group, portions Copyright (c) 1994 The Regents of
the University of California. The full PostgreSQL License text is at
https://www.postgresql.org/about/licence/ and ships with the installer (P35).
