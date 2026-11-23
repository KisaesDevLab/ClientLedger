# Notice

## What the license covers

`LICENSE.md` is the **VibeMyFirm Community License 1.0** (licensor Kisaes LLC,
Monett, Missouri, USA; <https://vibemyfirm.com/licenses/community-1.0>). It
covers ClientLedger: everything in this repository except the `engine/`
directory. Fonts, icons, and bundled binaries are listed with their own licenses
in `ATTRIBUTION.md`.

Versions of ClientLedger published before 2026-11-06 were under the PolyForm
Internal Use License 1.0.0 (and, before 2026-10-30, the MIT License); copies
obtained under those terms keep them.

## What that means for you

This is a plain-language summary; the license text is the controlling document.

- **Your own books are free.** Using ClientLedger to keep the books of a
  business or other organization you own, control, or work for needs no fee and
  no key, with no limit on users or companies (License §5.1).
- **Clients' books for pay need a Pro License Key** for your firm (License §5.2,
  §6). It covers everyone working for your firm, on any number of computers and
  for any number of clients.
- **Pro and Developer keys are perpetual for one major version** (License §9A).
  A key covers every release of the major version it names (for example every
  1.x release), whenever published. A later major version needs an Upgrade Key;
  ClientLedger never disables, locks, or deletes your data because a newer major
  version exists, and your own books never need a key in any version.
- **Until Pro keys can be bought and entered** (planned for the 1.0 release),
  Kisaes permits Professional Use of the 0.x test versions without a key.
  ClientLedger checks nothing.
- **Distributing ClientLedger, changed or not, needs a Developer License Key**
  (License §5.3, §7). A distribution may never include the engine, must keep
  the notices, must not use the ClientLedger, Vibe, VibeMyFirm, MyBooks or
  Kisaes names as its brand, and must say in its About screen: "Based on
  software © Kisaes LLC, licensed under the VibeMyFirm Community License."
- License keys are verified on your computer. ClientLedger never checks a key
  online, and never counts seats or companies, expires, or locks your data.

## The engine

ClientLedger runs on the Kisaes GL Engine (the Vibe MyBooks engine), added to
this repository as the `engine/` git submodule
(<https://github.com/KisaesDevLab/kisaes-gl-engine>; until 2026-11-08 it lived inside <https://github.com/KisaesDevLab/Vibe-MyBooks>). The engine is copyright
Kisaes LLC and is licensed separately: its source is under the PolyForm
Internal Use License 1.0.0 with the Kisaes Engine Notice, and ClientLedger is a
Grantee Product under the Kisaes Engine Product Grant
(`licensing/KISAES-GL-ENGINE-LICENSE-AND-GRANT.md`).

Inside an **official ClientLedger build** (built, signed, and published by
Kisaes LLC; until 1.0, the unsigned test versions Kisaes publishes with their
checksums on its release page also count as official builds), the engine is licensed to you under the VibeMyFirm Community
License. You may run that engine unchanged, as Kisaes ships it, for your
permitted purpose. You may not change, distribute, or host the engine for
others; that needs a Kisaes GL Engine OEM License
(<https://vibemyfirm.com/licenses/engine-oem>). The engine checks the signature
of the program that starts it; started by an unsigned or third-party program,
it runs only in internal mode.

Questions: licensing@kisaes.com.

## Non-affiliation

ClientLedger is an independent project. It is not affiliated with, endorsed by,
or sponsored by Intuit Inc. Intuit and QuickBooks are trademarks of Intuit Inc.,
mentioned here only to describe import and migration features. Other product
names are trademarks of their owners.

*Required Notice: Copyright © 2026 Kisaes LLC. Licensed under the VibeMyFirm Community License 1.0, https://vibemyfirm.com/licenses/community-1.0*
