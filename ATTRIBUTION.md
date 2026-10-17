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

PostgreSQL Database Management System, portions Copyright (c) 1996-2026 The
PostgreSQL Global Development Group, portions Copyright (c) 1994 The Regents of
the University of California. The full PostgreSQL License text is at
https://www.postgresql.org/about/licence/ and ships with the installer (P35).
