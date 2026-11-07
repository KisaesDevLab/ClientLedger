# Kisaes GL Engine — License, Product Grant, and Notice

Repository: `kisaes-gl-engine`
Copyright © 2026 Kisaes LLC, Monett, Missouri, USA

This repository ships three documents. Read all three.

| File | What it is |
| --- | --- |
| `LICENSE` | The source license for this repository: PolyForm Internal Use License 1.0.0, verbatim, as published at https://polyformproject.org/licenses/internal-use/1.0.0 |
| `ENGINE-GRANT.md` | The Kisaes Engine Product Grant (Part A below): the perpetual grant under which the Engine ships inside official Kisaes products |
| `NOTICE.md` | The Kisaes Engine Notice (Part B below): clarifications and the pointer to the OEM License for anyone who wants to distribute or host the Engine |

---

## Part A — Kisaes Engine Product Grant 1.0

### A.1 Purpose

Kisaes LLC ("Kisaes") owns the Kisaes GL Engine (the "Engine") and also owns the application products that embed it. Because grantor and grantee are the same legal entity, this Grant creates no obligation between separate parties today. It is published so that (a) users, forks, and contributors know exactly what rights travel with each product, and (b) if any product is ever transferred to another entity, the rights it needs are already defined.

### A.2 Grantee Products

The "Grantee Products" are:

1. ClientLedger (desktop bookkeeping);
2. ClientLedger Trial Balance (desktop trial balance);
3. Vibe MyBooks (self-hosted bookkeeping appliance, including the "MyBooks Online" edition); and
4. any successor, renamed, or additional product that Kisaes designates in writing as a Grantee Product by adding it to this list in the repository.

### A.3 Grant

Kisaes grants to each Grantee Product, and to whoever lawfully owns that product from time to time (the "Product Owner"), a perpetual, irrevocable, worldwide, royalty-free, non-exclusive license to:

1. embed the Engine, in source or binary form, in Official Builds of the Grantee Product;
2. distribute the Engine as part of those Official Builds under the Grantee Product's own end-user terms;
3. host the Engine as part of the Grantee Product for the Product Owner's customers; and
4. make changes to the Engine solely as needed to integrate it with the Grantee Product, provided such changes are offered back to the `kisaes-gl-engine` repository.

### A.4 What the Grant does not cover

1. **Forks of a Grantee Product.** The Grant runs to Official Builds only. A fork or modified build of a Grantee Product made by anyone other than the Product Owner receives no rights in the Engine from this Grant. Such a build may call an Engine binary its user obtained from Kisaes, under the VibeMyFirm Community License, but may not redistribute the Engine.
2. **Other products.** Embedding the Engine in any product that is not a Grantee Product requires a Kisaes GL Engine OEM License.
3. **Trademarks.** No trademark rights are granted.

### A.5 Transfer of a Grantee Product

If Kisaes sells, spins out, or otherwise transfers a Grantee Product to a different legal entity (the "Transferee"):

1. this Grant converts automatically, at closing, into a perpetual, irrevocable license from Kisaes to the Transferee on the terms of Section A.3, limited to that Grantee Product and its successors;
2. the Transferee receives the Engine source as of the closing date and the right to maintain its own fork for that product, with no obligation to contribute changes back;
3. no royalty is owed for the transferred Grantee Product; and
4. if the Transferee later embeds the Engine in a different product, that product requires an OEM License, and the then-current OEM rate applies.

### A.6 Reliability

Kisaes cannot revoke this Grant. Kisaes may add Grantee Products but may not remove one once a public release of that product has shipped with the Engine.

### A.7 Adoption

Adopted by the sole member of Kisaes LLC on 2026-11-06 by written resolution, a copy of which is kept in the company records. The resolution records that the `kisaes-gl-engine` repository and all rights in the Engine are assets of Kisaes LLC.

---

## Part B — Kisaes Engine Notice

### B.1 Internal use is free

The `LICENSE` file is the PolyForm Internal Use License 1.0.0. In short: you may use, build, and modify the Engine for the internal business operations of your company, at no charge, with no key. You may not distribute it.

### B.2 What "internal business operations" does not include

For the avoidance of doubt, and as a condition of the license Kisaes grants in `LICENSE`, the following are **not** internal business operations and require a Kisaes GL Engine OEM License:

1. including the Engine, modified or not, in any software you distribute to anyone outside your company, whether sold, free, open source, or white-labeled;
2. hosting the Engine, or any service built on it, so that persons outside your company use it, including software-as-a-service offerings, client portals, and multi-tenant platforms; and
3. selling, licensing, or offering access to the Engine's functionality under any brand.

Keeping the books of your own clients on your own internal installation, where your clients do not themselves log in to the Engine, remains internal business operations.

### B.3 Official Builds

The Engine inside an Official Build of ClientLedger, ClientLedger Trial Balance, or Vibe MyBooks is licensed to the end user under that product's VibeMyFirm Community License, not under `LICENSE`. The Engine checks the signature of the host application that launches it. An unsigned or third-party host runs the Engine only in internal mode (no distribution, no remote users) unless an OEM key is present.

### B.4 Contributions

Contributions are accepted only under the Kisaes Contributor License Agreement (`CLA.md`). The CLA lets Kisaes relicense contributed code to OEM licensees and to Product Owners under Part A.

### B.5 How to get an OEM License

See `OEM-LICENSE.md` or write to licensing@kisaes.com.

---

*Required Notice: Copyright © 2026 Kisaes LLC. The Kisaes GL Engine is licensed under the PolyForm Internal Use License 1.0.0 with the Kisaes Engine Notice. Distribution and hosting for others require an OEM License: https://vibemyfirm.com/licenses/engine-oem*
