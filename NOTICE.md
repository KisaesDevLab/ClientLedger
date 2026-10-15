# Notice

## What the MIT license covers

The `LICENSE` file (MIT) covers ClientLedger's own code: everything in this
repository except the `engine/` directory. Fonts, icons, and bundled binaries
are listed with their own licenses in `ATTRIBUTION.md`.

## The engine is not MIT

ClientLedger runs on the Vibe MyBooks engine, which is added to this repository
as the `engine/` git submodule
(<https://github.com/KisaesDevLab/vibe-mybooks>). The engine is licensed under
the **PolyForm Internal Use License 1.0.0** plus the **ClientLedger
Redistribution Grant**, which lives in the engine repository as
`LICENSE-CLIENTLEDGER.md`. Nothing in this repository's MIT license changes
those terms.

## Scope of the engine grant

- You may use the engine bundled in an official ClientLedger release to keep
  the books of **your own organization only**. Your outside accountant or
  bookkeeper may review, adjust, and receive those books. Keeping books for
  other people or organizations as a service needs a MyBooks commercial license.
- Only the Licensor's official releases (and personal builds from unmodified
  source, for your own use) may include the engine.
- **Forks of the MIT code may not bundle, download, or install the engine.**
  A modified or rebranded fork must not include or fetch the engine under the
  grant.

The grant text is the controlling document; this notice is a summary. The
grant is still in draft pending attorney review (see `docs/legal/`).

## Non-affiliation

ClientLedger is an independent project. It is not affiliated with, endorsed by,
or sponsored by Intuit Inc. QuickBooks is a trademark of Intuit Inc., mentioned
here only to describe import and migration features.
