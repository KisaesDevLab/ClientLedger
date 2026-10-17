# ClientLedger

Free, open-source, fully offline desktop bookkeeping for US small business
owners. Form-first entry, two-line registers, and keyboard-driven everything,
powered by the Vibe MyBooks engine.

When your accountant needs the books, ClientLedger exports a transfer file they
import into MyBooks Online.

- UI: Rust and egui; Windows, macOS, and Linux from one codebase.
- Database: PostgreSQL 16, bundled and managed by the app.
- Network: none required. AI suggestions and invoice email are optional and
  configured by the owner.

**Status:** under construction (pre-beta). See `STATE.md` and `PHASES.md`.

## Licensing

- ClientLedger code: MIT, see [LICENSE](LICENSE).
- The bundled engine in `engine/`: PolyForm Internal Use License 1.0.0, see
  [NOTICE.md](NOTICE.md). Recipients may not distribute it, and forks may not
  bundle, download, or install the engine.

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
