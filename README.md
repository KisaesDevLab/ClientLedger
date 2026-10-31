# ClientLedger

Free, source-available, fully offline desktop bookkeeping for US small business
owners. Form-first entry, two-line registers, and keyboard-driven everything,
powered by the Vibe MyBooks engine.

When your accountant needs the books, ClientLedger exports a transfer file they
import into MyBooks Online.

- UI: Rust and egui; Windows, macOS, and Linux from one codebase.
- Database: PostgreSQL 16, bundled and managed by the app.
- Network: none required. AI suggestions and invoice email are optional and
  configured by the owner.

**Status:** under construction (pre-beta). See `STATE.md` and `PHASES.md`.

## Trying a preview build

Unsigned preview builds for testing are made automatically each time a phase
merges to `main`, and on demand. In the repository's **Actions** tab open the
**preview** workflow, pick a run, and download the archive for your system from
**Artifacts** (Windows `.zip`, macOS Apple silicon and Intel `.zip`, Linux
`.tar.gz`, each with a `.sha256` file). Artifacts are kept for 30 days. Unpack
the folder and follow `README-PREVIEW.txt` inside it.

Preview builds are **not signed**: Windows SmartScreen and macOS Gatekeeper warn
about an unknown developer, and `README-PREVIEW.txt` explains how to open them.
They are for trying the app only; do not keep real books in them. Developers can
build the same archive locally with `cargo xtask preview`. Signed installers
come later (phase P35).

## Licensing

- ClientLedger code: PolyForm Internal Use License 1.0.0, see
  [LICENSE](LICENSE). Use it, and change it, for your own organization's
  internal business; it may not be distributed.
- The bundled engine in `engine/`: a commercial license from Kisaes LLC that is
  included with ClientLedger at no charge and with no license key. See
  [NOTICE.md](NOTICE.md). Only official releases include it.

## Contributing

Issues and discussions are welcome. Pull requests from outside the
organization are not accepted and are closed automatically; see
[CONTRIBUTING.md](CONTRIBUTING.md). Security reports: [SECURITY.md](SECURITY.md).

## Development

All tasks run through `cargo xtask`; see `CLAUDE.md` for the command list.

```
cargo xtask ci
```

## Repository settings (owner applies on GitHub)

These cannot be set from the repository and are applied by the owner:

- Protect `main`: require the `ci` status checks (Linux, Windows, macOS) to pass
  before merging, and require pull requests.
- Enable "Allow auto-merge" so approved-by-CI pull requests merge themselves.
- Enable secret scanning and push protection.
- Turn on Issues and Discussions.

## Trademarks

QuickBooks and Intuit are trademarks of Intuit Inc. ClientLedger is an
independent project and is not affiliated with, endorsed by, or sponsored by
Intuit Inc. QuickBooks is mentioned only to describe import and migration
features. Other product names are trademarks of their owners.
