"""Generates the synthetic bank fixtures for P25 (fixtures/bank) and
expected.json, the oracle for crates/cl-import/tests/fixtures.rs. All data
is invented. Expected counts, sums and lines are computed here, independently
of cl-import. Run from anywhere: python -I fixtures/bank/generate.py <repo root>
(dedupe_v1.json is not generated: it is a byte copy of the engine's golden file)."""
import json, sys, os
from decimal import Decimal as D

ROOT = sys.argv[1]
OUT = os.path.join(ROOT, "fixtures", "bank")
MAL = os.path.join(OUT, "malformed")
os.makedirs(MAL, exist_ok=True)
EXPECTED = {"files": [], "malformed": []}
MASK = "••••"


def write(name, data: bytes):
    with open(os.path.join(OUT, name), "wb") as f:
        f.write(data)


def wmal(name, data: bytes):
    with open(os.path.join(MAL, name), "wb") as f:
        f.write(data)


def mask(acct):
    n = min(len(acct) // 2, 4)
    return MASK + (acct[-n:] if n else "")


def iso(d):  # "20261103" -> "2026-11-03"
    return f"{d[0:4]}-{d[4:6]}-{d[6:8]}"


def us(d):  # "20261103" -> "11/03/2026"
    return f"{d[4:6]}/{d[6:8]}/{d[0:4]}"


# ---------------------------------------------------------------- OFX
# txn: dict(fitid, date 'YYYYMMDD', amt str, type, name, memo?, check?, refnum?, dtsuffix?)

def stmttrn_sgml(t, nl):
    parts = ["<STMTTRN>", f"<TRNTYPE>{t['type']}", f"<DTPOSTED>{t['date']}{t.get('dtsuffix', '120000')}",
             f"<TRNAMT>{t['amt']}", f"<FITID>{t['fitid']}"]
    if t.get("check"):
        parts.append(f"<CHECKNUM>{t['check']}")
    if t.get("refnum"):
        parts.append(f"<REFNUM>{t['refnum']}")
    parts.append(f"<NAME>{t['name']}")
    if t.get("memo"):
        parts.append(f"<MEMO>{t['memo']}")
    parts.append("</STMTTRN>")
    return nl.join(parts)


def stmttrn_xml(t, nl):
    def esc(s):
        return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    parts = ["<STMTTRN>", f"<TRNTYPE>{t['type']}</TRNTYPE>",
             f"<DTPOSTED>{t['date']}{t.get('dtsuffix', '120000')}</DTPOSTED>",
             f"<TRNAMT>{t['amt']}</TRNAMT>", f"<FITID>{t['fitid']}</FITID>"]
    if t.get("check"):
        parts.append(f"<CHECKNUM>{t['check']}</CHECKNUM>")
    if t.get("refnum"):
        parts.append(f"<REFNUM>{t['refnum']}</REFNUM>")
    parts.append(f"<NAME>{esc(t['name'])}</NAME>")
    if t.get("memo"):
        parts.append(f"<MEMO>{esc(t['memo'])}</MEMO>")
    parts.append("</STMTTRN>")
    return nl.join(parts)


def stmt_body(stmt, xml, nl):
    """stmt: dict(card, bankid, acctid, accttype, start, end, txns, bal, baldate)"""
    c = (lambda tag, v: f"<{tag}>{v}</{tag}>") if xml else (lambda tag, v: f"<{tag}>{v}")
    trn = stmttrn_xml if xml else stmttrn_sgml
    lines = []
    if stmt.get("card"):
        lines += ["<CREDITCARDMSGSRSV1>", "<CCSTMTTRNRS>", c("TRNUID", "1001"), "<STATUS>", c("CODE", "0"),
                  c("SEVERITY", "INFO"), "</STATUS>", "<CCSTMTRS>", c("CURDEF", "USD"), "<CCACCTFROM>",
                  c("ACCTID", stmt["acctid"]), "</CCACCTFROM>"]
    else:
        lines += ["<STMTTRNRS>", c("TRNUID", "1001"), "<STATUS>", c("CODE", "0"), c("SEVERITY", "INFO"),
                  "</STATUS>", "<STMTRS>", c("CURDEF", "USD"), "<BANKACCTFROM>", c("BANKID", stmt["bankid"]),
                  c("ACCTID", stmt["acctid"]), c("ACCTTYPE", stmt.get("accttype", "CHECKING")), "</BANKACCTFROM>"]
    lines += ["<BANKTRANLIST>", c("DTSTART", stmt["start"]), c("DTEND", stmt["end"])]
    for t in stmt["txns"]:
        lines.append(trn(t, nl))
    lines += ["</BANKTRANLIST>"]
    if stmt.get("bal") is not None:
        lines += ["<LEDGERBAL>", c("BALAMT", stmt["bal"]), c("DTASOF", stmt["baldate"]), "</LEDGERBAL>"]
    if stmt.get("card"):
        lines += ["</CCSTMTRS>", "</CCSTMTTRNRS>", "</CREDITCARDMSGSRSV1>"]
    else:
        lines += ["</STMTRS>", "</STMTTRNRS>"]
    return lines


def ofx(stmts, xml=False, qfx=False, charset="1252", encoding="USASCII", xml_enc="UTF-8", nl="\r\n",
        bank_msgs=True):
    c = (lambda tag, v: f"<{tag}>{v}</{tag}>") if xml else (lambda tag, v: f"<{tag}>{v}")
    if xml:
        head = [f'<?xml version="1.0" encoding="{xml_enc}" standalone="no"?>',
                '<?OFX OFXHEADER="200" VERSION="220" SECURITY="NONE" OLDFILEUID="NONE" NEWFILEUID="NONE"?>']
    else:
        head = ["OFXHEADER:100", "DATA:OFXSGML", "VERSION:102", "SECURITY:NONE", f"ENCODING:{encoding}",
                f"CHARSET:{charset}", "COMPRESSION:NONE", "OLDFILEUID:NONE", "NEWFILEUID:NONE", ""]
    body = ["<OFX>", "<SIGNONMSGSRSV1>", "<SONRS>", "<STATUS>", c("CODE", "0"), c("SEVERITY", "INFO"), "</STATUS>",
            c("DTSERVER", "20261103120000"), c("LANGUAGE", "ENG"), "<FI>", c("ORG", "Ozark Valley Bank"),
            c("FID", "40404"), "</FI>"]
    if qfx:
        body.append(c("INTU.BID", "40404"))
    body += ["</SONRS>", "</SIGNONMSGSRSV1>"]
    bank = [s for s in stmts if not s.get("card")]
    card = [s for s in stmts if s.get("card")]
    if bank:
        body.append("<BANKMSGSRSV1>")
        for s in bank:
            body += stmt_body(s, xml, nl)
        body.append("</BANKMSGSRSV1>")
    for s in card:
        body += stmt_body(s, xml, nl)
    body.append("</OFX>")
    return nl.join(head + body) + nl


def stmttrn_lines(stmts, xml, qfx, **kw):
    """Line numbers of each <STMTTRN> in the generated text."""
    text = ofx(stmts, xml=xml, qfx=qfx, **kw)
    lines = text.replace("\r\n", "\n").split("\n")
    return [i + 1 for i, l in enumerate(lines) if l.startswith("<STMTTRN>")]


def ofx_desc(name, memo):
    # Owner decision Q-P25r (2026-11-07): NAME only; MEMO goes to the memo.
    return (name or "").strip(" \t\n\r\x0b\x0c\xa0")


def expect_ofx(file, fmt, stmts, sections_extra=None, encoding=None):
    secs = []
    for i, s in enumerate(stmts):
        rows = [t for t in s["txns"] if D(t["amt"]) != 0]
        sec = {
            "kind": "CreditCard" if s.get("card") else {"CHECKING": "Checking", "SAVINGS": "Savings"}[
                s.get("accttype", "CHECKING")],
            "mask": mask(s["acctid"]),
            "rows": len(rows),
            "sum": str(sum((D(t["amt"]) for t in rows), D(0))),
            "start": iso(s["start"]),
            "end": iso(s["end"]),
            "balance": s.get("bal"),
            "balanceDate": iso(s["baldate"]) if s.get("baldate") else None,
            "problems": [],
            "skipped": [],
            "firstDescription": ofx_desc(rows[0]["name"], rows[0].get("memo")) if rows else None,
        }
        if sections_extra and i < len(sections_extra):
            sec.update(sections_extra[i])
        secs.append(sec)
    e = {"file": file, "format": fmt, "sections": secs}
    if encoding:
        e["encoding"] = encoding
    EXPECTED["files"].append(e)


# 1. checking_ofx102_sgml.ofx (CHARSET:NONE; raw & and a 1252 byte)
chk_txns = [
    dict(fitid="CHK20261002A", date="20261002", amt="-45.10", type="POS", name="POS PURCHASE COFFEE CO", memo="coffee co"),
    dict(fitid="CHK20261003A", date="20261003", amt="2500.00", type="DIRECTDEP", name="PAYROLL DEPOSIT", memo="ACME WIDGETS"),
    dict(fitid="CHK20261005A", date="20261005", amt="-120.00", type="CHECK", name="CHECK", check="1001"),
    dict(fitid="CHK20261006A", date="20261006", amt="-89.99", type="DEBIT", name="AT&T WIRELESS AUTOPAY"),
    dict(fitid="CHK20261008A", date="20261008", amt="-12.50", type="POS", name="CAF\xc9 LUMI\xc8RE", memo="TABLE 4"),
    dict(fitid="CHK20261010A", date="20261010", amt="-60.00", type="ATM", name="ATM WITHDRAWAL", refnum="ATM0042"),
    dict(fitid="CHK20261012A", date="20261012", amt="-1350.00", type="CHECK", name="CHECK", check="1002", memo="RENT OCTOBER"),
    dict(fitid="CHK20261015A", date="20261015", amt="415.25", type="CREDIT", name="MOBILE DEPOSIT"),
    dict(fitid="CHK20261017A", date="20261017", amt="-5.00", type="SRVCHG", name="MONTHLY SERVICE FEE"),
    dict(fitid="CHK20261020A", date="20261020", amt="-230.18", type="PAYMENT", name="CITY UTILITIES"),
    dict(fitid="CHK20261024A", date="20261024", amt="-18.75", type="POS", name="HARDWARE &amp; GARDEN"),
    dict(fitid="CHK20261028A", date="20261028", amt="0.38", type="INT", name="INTEREST PAID"),
]
chk = dict(bankid="021000021", acctid="000123456789", start="20261001", end="20261031", txns=chk_txns,
           bal="5234.61", baldate="20261031")
text = ofx([chk], charset="NONE", encoding="USASCII")
write("checking_ofx102_sgml.ofx", text.encode("cp1252"))
lines1 = stmttrn_lines([chk], False, False, charset="NONE", encoding="USASCII")
expect_ofx("checking_ofx102_sgml.ofx", "Ofx", [chk], [{"rowLines": lines1[:3],
           "descriptions": {"4": "AT&T WIRELESS AUTOPAY", "5": "CAFÉ LUMIÈRE",
                            "7": "CHECK", "11": "HARDWARE & GARDEN", "1": "POS PURCHASE COFFEE CO"},
           "checks": {"3": 1001, "7": 1002}, "memos": {"1": "coffee co", "2": "ACME WIDGETS", "5": "TABLE 4", "6": "Ref: ATM0042", "7": "RENT OCTOBER"}}], encoding="Windows1252")
# Sum check by hand is in expected.json.

# 2. checking_ofx22_xml.ofx (ISO-8859-1, [-8:PST] and [+12])
x_txns = [
    dict(fitid="X1", date="20261101", amt="-42.00", type="DEBIT", name="GROCERY OUTLET", dtsuffix="235959.000[-8:PST]"),
    dict(fitid="X2", date="20261101", amt="1200.00", type="CREDIT", name="CLIENT PAYMENT", dtsuffix="000000[+12]"),
    dict(fitid="X3", date="20261102", amt="-7.25", type="POS", name="CR\xc8ME BAKERY", dtsuffix="[+12:NZST]"),
    dict(fitid="X4", date="20261103", amt="-300.00", type="XFER", name="TRANSFER TO SAVINGS", dtsuffix=""),
    dict(fitid="X5", date="20261104", amt="-64.12", type="DEBIT", name="FUEL STOP", memo="PUMP 3", dtsuffix="120000.000[-5:EST]"),
    dict(fitid="X6", date="20261105", amt="-15.99", type="DEBIT", name="STREAMING SVC", dtsuffix="080000[-8:PST]"),
    dict(fitid="X7", date="20261106", amt="88.40", type="CREDIT", name="REFUND <ONLINE>", dtsuffix="000000[0:GMT]"),
    dict(fitid="X8", date="20261107", amt="-1.50", type="FEE", name="ATM FEE", dtsuffix="235959[-10:HST]"),
]
xs = dict(bankid="111000025", acctid="9876543210", start="20261101", end="20261107", txns=x_txns, bal="3120.33",
          baldate="20261107")
text = ofx([xs], xml=True, xml_enc="ISO-8859-1", nl="\n")
write("checking_ofx22_xml.ofx", text.encode("latin-1"))
expect_ofx("checking_ofx22_xml.ofx", "Ofx", [xs], [{"descriptions": {"3": "CRÈME BAKERY", "7": "REFUND <ONLINE>"},
                                                     "dates": {"1": "2026-11-01", "2": "2026-11-01", "8": "2026-11-07"}}],
           encoding="Windows1252")

# 3. operating_2026-11-03.qfx (mockup: 41 rows, 92,431.07; account ••••2208)
OP_ACCT = "4410082208"
mock = [
    ("20261028", "8450.00", "ACH CREDIT BRANSON HILLS HOA", None, "CREDIT"),
    ("20261030", "-48.00", "SERVICE CHARGE", None, "SRVCHG"),
    ("20261031", "-186.40", "MONETT TIRE AND LUBE 0331", None, "POS"),
    ("20261031", "-45.00", "SQ *FARMERS MKT BOOTH", None, "POS"),
    ("20261101", "-301.22", "LAKESIDE POWER COOP AUTOPAY", None, "DIRECTDEBIT"),
    ("20261101", "-64.38", "OZARK OFFICE SUPPLY 0045", None, "POS"),
    ("20261102", "-925.00", "CHECK", "4412", "CHECK"),
    ("20261102", "-3000.00", "ONLINE TRANSFER TO XXXX4471", None, "XFER"),
    ("20261103", "8450.00", "ACH CREDIT BRANSON HILLS HOA", None, "CREDIT"),
    ("20261103", "-84.17", "RIVERBEND FUEL #212 MONETT MO", None, "POS"),
]
extra_names = ["SQ *MAIN ST DELI", "CASEY'S GENERAL STORE", "AMAZON MKTPLACE PMTS", "USPS PO 2840", "OREILLY AUTO 0412",
               "WALMART SUPERCENTER 5521", "GOOGLE WORKSPACE", "INTUIT-FREE GAS", "DOLLAR GENERAL 1180", "SHELL OIL 5741",
               "BRANSON TIRE CO", "MERCHANT DEPOSIT", "SQUARE INC DEPOSIT", "HARBOR FREIGHT 309"]
extra = []
dates = ["20261028", "20261029", "20261029", "20261030", "20261030", "20261031", "20261031", "20261101", "20261101",
         "20261102", "20261102", "20261103", "20261103"]
amts_cycle = ["-12.48", "-56.20", "-23.99", "-8.75", "-140.00", "-71.36", "-19.99", "1840.00", "-33.10", "-9.42",
              "-210.57", "975.25", "-44.44", "-17.80"]
for i in range(31):
    d = dates[i % len(dates)]
    nm = extra_names[i % len(extra_names)]
    a = amts_cycle[i % len(amts_cycle)]
    typ = "CREDIT" if not a.startswith("-") else "POS"
    extra.append((d, a, nm, None, typ))
op_rows = sorted(mock + extra, key=lambda r: r[0])
op_txns = []
for i, (d, a, nm, chkno, typ) in enumerate(op_rows):
    op_txns.append(dict(fitid=f"OVB{d}{i:04d}", date=d, amt=a, type=typ, name=nm, check=chkno))
# The four rows already imported by operating_prior.qfx: the 10/28 HOA credit,
# the 10/30 service charge, and the first two extra 10/29 rows.
dup_idx = [i for i, t in enumerate(op_txns) if (t["date"], t["name"]) in
           {("20261028", "ACH CREDIT BRANSON HILLS HOA"), ("20261030", "SERVICE CHARGE")}]
dup_idx += [i for i, t in enumerate(op_txns) if t["date"] == "20261029"][:2]
for i in dup_idx:
    op_txns[i]["fitid"] = f"OVBPRIOR{op_txns[i]['date']}{i:02d}"
op = dict(bankid="081000032", acctid=OP_ACCT, start="20261028", end="20261103", txns=op_txns, bal="92431.07",
          baldate="20261103")
text = ofx([op], xml=True, qfx=True, nl="\n")
write("operating_2026-11-03.qfx", text.encode("utf-8"))
assert len(op_txns) == 41
expect_ofx("operating_2026-11-03.qfx", "Qfx", [op], encoding="Utf8")

# 4. operating_prior.qfx: 10/20-10/30, contains the 4 duplicates.
prior_txns = [dict(fitid=f"OVBP{d}{i}", date=d, amt=a, type="POS" if a.startswith("-") else "CREDIT", name=n)
              for i, (d, a, n) in enumerate([
                  ("20261020", "-35.00", "SQ *MAIN ST DELI"), ("20261021", "-410.00", "OZARK OFFICE SUPPLY 0045"),
                  ("20261023", "6200.00", "SQUARE INC DEPOSIT"), ("20261024", "-72.15", "SHELL OIL 5741"),
                  ("20261027", "-118.30", "WALMART SUPERCENTER 5521")])]
prior_txns += [dict(op_txns[i]) for i in sorted(dup_idx)]
prior_txns.sort(key=lambda t: t["date"])
prior = dict(bankid="081000032", acctid=OP_ACCT, start="20261020", end="20261030", txns=prior_txns, bal="88000.00",
             baldate="20261030")
write("operating_prior.qfx", ofx([prior], xml=True, qfx=True, nl="\n").encode("utf-8"))
expect_ofx("operating_prior.qfx", "Qfx", [prior], encoding="Utf8")

# 5/6. card fixtures, both conventions (same transactions).
card_rows = [("20261005", "-45.67", "DEBIT", "OFFICE DEPOT #221"), ("20261007", "-12.00", "DEBIT", "PARKING GARAGE"),
             ("20261010", "-230.40", "DEBIT", "AIRLINE TICKET"), ("20261012", "500.00", "CREDIT", "PAYMENT THANK YOU"),
             ("20261015", "-89.10", "DEBIT", "HOTEL STAY"), ("20261018", "-15.25", "DEBIT", "LUNCH MEETING"),
             ("20261022", "25.00", "CREDIT", "RETURN OFFICE DEPOT"), ("20261025", "-61.80", "DEBIT", "FUEL")]
def card_stmt(positive):
    txns = [dict(fitid=f"CC{d}{i}", date=d, amt=(a[1:] if a.startswith("-") else "-" + a) if positive else a,
                 type=t, name=n) for i, (d, a, t, n) in enumerate(card_rows)]
    return dict(card=True, acctid="4111000011112222", start="20261001", end="20261031", txns=txns,
                bal="429.12" if positive else "-429.12", baldate="20261031")
neg_card, pos_card = card_stmt(False), card_stmt(True)
write("card_ccstmt.qfx", ofx([neg_card], qfx=True).encode("ascii"))
write("card_positive_issuer.qfx", ofx([pos_card], qfx=True).encode("ascii"))
expect_ofx("card_ccstmt.qfx", "Qfx", [neg_card], [{"suggestFlip": False}], encoding="Utf8")
expect_ofx("card_positive_issuer.qfx", "Qfx", [pos_card], [{"suggestFlip": True}], encoding="Utf8")

# 7. multi_account.ofx: checking, savings, card.
m_chk = dict(bankid="021000021", acctid="111122223333", start="20261101", end="20261105", bal="1500.00",
             baldate="20261105", txns=[dict(fitid="M1", date="20261101", amt="-20.00", type="POS", name="CAFE"),
                                      dict(fitid="M2", date="20261103", amt="-300.00", type="XFER", name="TO SAVINGS")])
m_sav = dict(bankid="021000021", acctid="111122224444", accttype="SAVINGS", start="20261101", end="20261105",
             bal="10300.00", baldate="20261105",
             txns=[dict(fitid="S1", date="20261103", amt="300.00", type="XFER", name="FROM CHECKING"),
                   dict(fitid="S2", date="20261105", amt="1.12", type="INT", name="INTEREST")])
m_card = dict(card=True, acctid="5500001234", start="20261101", end="20261105", bal="-75.00", baldate="20261105",
              txns=[dict(fitid="C1", date="20261102", amt="-75.00", type="DEBIT", name="SOFTWARE SUBSCRIPTION")])
write("multi_account.ofx", ofx([m_chk, m_sav, m_card]).encode("ascii"))
expect_ofx("multi_account.ofx", "Ofx", [m_chk, m_sav, m_card], encoding="Utf8")

# ---------------------------------------------------------------- QIF
qif_rows = [(" 1/ 5/26", "-12.50", "COFFEE HOUSE", "morning", "1001"), (" 1/ 6/26", "1,250.00", "CLIENT A", None, None),
            ("1/7'26", "-60.00", "ATM", None, "ATM"), ("01/09/2026", "-310.45", "LANDLORD LLC", "Jan rent", "1002"),
            ("1/12/26", "-19.99", "WEB HOSTING", None, None), ("1/15/26", "-0.00", "VOID CHECK", None, "1003"),
            ("1/20/26", "475.00", "CLIENT B", None, None), ("1/22/26", "-88.10", "OFFICE STORE", None, None),
            ("1/28/26", "-5.00", "BANK FEE", None, None), ("1/30/26", "-140.00", "INSURANCE CO", None, None)]
lines = ["!Type:Cat", "NOffice", "DOffice expenses", "E", "^", "NRent", "E", "^", "!Type:Bank"]
qif_lines = []
for i, (d, a, p, m, n) in enumerate(qif_rows):
    qif_lines.append(len(lines) + 1)
    lines += [f"D{d}", f"T{a}", f"P{p}"]
    if m:
        lines.append(f"M{m}")
    if n:
        lines.append(f"N{n}")
    lines.append("LOffice")
    if i < len(qif_rows) - 1:
        lines.append("^")
# no final ^
write("checking.qif", ("\r\n".join(lines) + "\r\n").encode("ascii"))
q_nonzero = [(r, l) for r, l in zip(qif_rows, qif_lines) if D(r[1].replace(",", "")) != 0]
EXPECTED["files"].append({"file": "checking.qif", "format": "Qif", "sections": [{
    "kind": "Checking", "mask": None, "rows": len(q_nonzero),
    "sum": str(sum(D(r[1].replace(",", "")) for r, _ in q_nonzero)),
    "start": "2026-01-05", "end": "2026-01-30", "balance": None, "balanceDate": None, "problems": [],
    "skipped": [qif_lines[5]], "rowLines": [l for _, l in q_nonzero][:3],
    "checks": {"1": 1001}, "memos": {"1": "morning", "3": "Ref: ATM"}}]})

cq = [("10/03/2026", "-54.20", "BOOKSTORE"), ("10/05/2026", "-12.00", "PARKING"), ("10/09/2026", "300.00", "PAYMENT"),
      ("10/11/2026", "-77.77", "PRINT SHOP"), ("10/19/2026", "-9.99", "APP STORE")]
lines = ["!Account", "NBusiness Card", "TCCard", "^", "!Type:CCard"]
for d, a, p in cq:
    lines += [f"D{d}", f"T{a}", f"P{p}", "^"]
write("card.qif", ("\n".join(lines) + "\n").encode("ascii"))
EXPECTED["files"].append({"file": "card.qif", "format": "Qif", "sections": [{
    "kind": "CreditCard", "mask": None, "rows": 5, "sum": str(sum(D(a) for _, a, _ in cq)),
    "start": "2026-10-03", "end": "2026-10-19", "balance": None, "balanceDate": None, "problems": [], "skipped": [],
    "suggestFlip": False, "accountName": "Business Card"}]})

# Quicken's first record: "Opening Balance", a transfer to the account itself.
# Skipped (not a transaction); its amount and date become the opening balance.
ob = ["!Account", "NOperating", "TBank", "^", "!Type:Bank",
      "D12/31/25", "T4,210.55", "CX", "POpening Balance", "L[Operating]", "^",
      "D1/2/26", "T-25.00", "PPOST OFFICE", "LPostage", "^",
      "D1/3/26", "T300.00", "PCLIENT C", "^"]
write("opening_balance.qif", ("\r\n".join(ob) + "\r\n").encode("ascii"))
EXPECTED["files"].append({"file": "opening_balance.qif", "format": "Qif", "sections": [{
    "kind": "Checking", "mask": None, "rows": 2, "sum": "275.00", "start": "2026-01-02", "end": "2026-01-03",
    "balance": None, "balanceDate": None, "problems": [], "skipped": [6], "rowLines": [12, 17],
    "openingBalance": "4210.55", "openingBalanceDate": "2025-12-31", "accountName": "Operating"}]})

# ---------------------------------------------------------------- CSV
def csv_bytes(rows, enc="utf-8", bom=False, nl="\r\n", sep=","):
    def q(v):
        return f'"{v}"' if (sep in v or '"' in v) else v
    text = nl.join(sep.join(q(c) for c in r) for r in rows) + nl
    if enc == "utf-16-le":
        return b"\xff\xfe" + text.encode("utf-16-le")
    data = text.encode(enc)
    return (b"\xef\xbb\xbf" + data) if bom else data


def expect_csv(file, data_rows, amount_of, start, end, extra=None, header=True):
    nonzero = [(i, r) for i, r in enumerate(data_rows) if amount_of(r) is not None and amount_of(r) != 0]
    first = 2 if header else 1
    sec = {"kind": "Unknown", "mask": None, "rows": len(nonzero),
           "sum": str(sum((amount_of(r) for _, r in nonzero), D(0))),
           "start": start, "end": end, "balance": None, "balanceDate": None, "problems": [],
           "skipped": [first + i for i, r in enumerate(data_rows) if amount_of(r) == 0]}
    if extra:
        sec.update(extra)
    EXPECTED["files"].append({"file": file, "format": "Csv", "sections": [sec]})


base10 = [["11/01/2026", "Café Lumière", "-12.50"], ["11/01/2026", "CLIENT PAYMENT INV 1001", "1,500.00"],
          ["11/02/2026", "OFFICE SUPPLY “DEPOT”", "-89.95"], ["11/03/2026", "UTILITY CO", "-142.08"],
          ["11/03/2026", "FUEL STATION #9", "-45.10"], ["11/04/2026", "SOFTWARE, INC.", "-29.00"],
          ["11/05/2026", "TRANSFER FROM SAVINGS", "500.00"], ["11/05/2026", "BANK FEE", "-3.00"],
          ["11/06/2026", "LUNCH – CLIENT", "-38.40"], ["11/07/2026", "REFUND", "12.99"]]
amt3 = lambda r: D(r[2].replace(",", ""))
write("signed_amount_bom.csv", csv_bytes([["Date", "Description", "Amount"]] + base10, bom=True))
expect_csv("signed_amount_bom.csv", base10, amt3, "2026-11-01", "2026-11-07",
           {"descriptions": {"1": "Café Lumière", "6": "SOFTWARE, INC."}})
write("win1252.csv", csv_bytes([["Date", "Description", "Amount"]] + base10, enc="cp1252"))
expect_csv("win1252.csv", base10, amt3, "2026-11-01", "2026-11-07", {"descriptions": {"1": "Café Lumière"}})
write("utf16le_tab.csv", csv_bytes([["Date", "Description", "Amount"]] + base10, enc="utf-16-le", sep="\t"))
expect_csv("utf16le_tab.csv", base10, amt3, "2026-11-01", "2026-11-07")

# 11. debit/credit with balance, debits written negative, oldest first.
dc = []
bal = D("1000.00")
for d, desc, debit, credit in [("10/01/2026", "OPENING DEPOSIT", "", "250.00"), ("10/02/2026", "COFFEE", "-4.50", ""),
                               ("10/03/2026", "SUPPLIES", "-120.25", ""), ("10/04/2026", "CLIENT A", "", "980.00"),
                               ("10/06/2026", "RENT", "-1,100.00", ""), ("10/07/2026", "FEE", "-2.00", ""),
                               ("10/09/2026", "CLIENT B", "", "310.10"), ("10/10/2026", "INTERNET", "-79.99", "")]:
    amt = D(credit.replace(",", "")) if credit else -abs(D(debit.replace(",", "")))
    bal += amt
    dc.append([d, desc, debit, credit, f"{bal:,.2f}"])
dc_amt = lambda r: D(r[3].replace(",", "")) if r[3] else -abs(D(r[2].replace(",", "")))
write("debit_credit_balance.csv", csv_bytes([["Posting Date", "Description", "Debit", "Credit", "Balance"]] + dc))
expect_csv("debit_credit_balance.csv", dc, dc_amt, "2026-10-01", "2026-10-10",
           {"balance": str(bal), "balanceDate": "2026-10-10", "continuity": "OldestFirst", "suggestFlip": False})

# 12. DR/CR indicator.
ind = [["10/01/2026", "PAYROLL", "2000.00", "CR"], ["10/02/2026", "RENT", "1200.00", "DR"],
       ["10/03/2026", "COFFEE", "4.75", "DR"], ["10/05/2026", "REFUND", "15.00", "CR"],
       ["10/06/2026", "TOOLS", "250.40", "DR"], ["10/08/2026", "INTEREST", "0.12", "CR"]]
ind_amt = lambda r: D(r[2]) if r[3] == "CR" else -D(r[2])
write("drcr_indicator.csv", csv_bytes([["Date", "Description", "Amount", "CR/DR"]] + ind))
expect_csv("drcr_indicator.csv", ind, ind_amt, "2026-10-01", "2026-10-08")

# 13. newest first with balance.
nf_old_first = [("09/01/2026", "DEPOSIT", "500.00"), ("09/02/2026", "GROCERIES", "-82.33"),
                ("09/04/2026", "GAS", "-40.00"), ("09/07/2026", "CLIENT", "1,200.00"), ("09/09/2026", "PHONE", "-65.00")]
bal = D("2000.00")
rows = []
for d, desc, a in nf_old_first:
    bal += D(a.replace(",", ""))
    rows.append([d, desc, a, f"{bal:.2f}"])
end_bal = bal
rows.reverse()
write("newest_first_balance.csv", csv_bytes([["Date", "Description", "Amount", "Running Balance"]] + rows))
expect_csv("newest_first_balance.csv", rows, lambda r: D(r[2].replace(",", "")), "2026-09-01", "2026-09-09",
           {"balance": str(end_bal), "balanceDate": "2026-09-09", "continuity": "NewestFirst", "suggestFlip": False})

# 14. card CSV with positive charges.
cp = [["10/02/2026", "OFFICE DEPOT", "45.67"], ["10/04/2026", "PARKING", "12.00"], ["10/06/2026", "PAYMENT", "-500.00"],
      ["10/08/2026", "HOTEL", "89.10"], ["10/09/2026", "LUNCH", "15.25"]]
write("card_positive_charges.csv", csv_bytes([["Trans. Date", "Description", "Amount"]] + cp))
expect_csv("card_positive_charges.csv", cp, lambda r: D(r[2]), "2026-10-02", "2026-10-09", {"cardSuggestFlip": True})

# 17. identical rows the same day.
ident = [["11/02/2026", "COFFEE CART", "-4.50"], ["11/02/2026", "COFFEE CART", "-4.50"],
         ["11/02/2026", "COFFEE CART", "-4.50"], ["11/03/2026", "BAGEL SHOP", "-6.25"]]
write("identical_same_day.csv", csv_bytes([["Date", "Description", "Amount"]] + ident))
expect_csv("identical_same_day.csv", ident, amt3, "2026-11-02", "2026-11-03", {"ordinals": [1, 2, 3, 1]})

# 18. overlap with fixture 10 (last 4 rows) plus 3 new.
ov = base10[6:] + [["11/08/2026", "PARKING METER", "-2.00"], ["11/09/2026", "CLIENT PAYMENT INV 1002", "750.00"],
                   ["11/10/2026", "POSTAGE", "-11.60"]]
write("overlap.csv", csv_bytes([["Date", "Description", "Amount"]] + ov))
expect_csv("overlap.csv", ov, amt3, "2026-11-05", "2026-11-10")

# 19. ambiguous dates (all read both ways).
amb = [["03/04/2026", "A", "-1.00"], ["05/06/2026", "B", "-2.00"], ["07/08/2026", "C", "-3.00"],
       ["01/01/2026", "D", "-4.00"]]
write("dates_ambiguous.csv", csv_bytes([["Date", "Description", "Amount"]] + amb))
expect_csv("dates_ambiguous.csv", amb, amt3, "2026-01-01", "2026-07-08", {"ambiguous": True, "dayFirstLine": None})

# 20. day/month dates: 13/02 cannot be month/day; 02/30 is no date at all.
ddmm = [["03/02/2026", "A", "-1.00"], ["13/02/2026", "B", "-2.00"], ["25/02/2026", "C", "-3.00"],
        ["02/30/2026", "D", "-4.00"], ["01/03/2026", "E", "-5.00"]]
write("dates_ddmm_impossible.csv", csv_bytes([["Date", "Description", "Amount"]] + ddmm))
EXPECTED["files"].append({"file": "dates_ddmm_impossible.csv", "format": "Csv", "sections": [{
    "kind": "Unknown", "mask": None, "rows": 2, "sum": "-6.00", "start": "2026-01-03", "end": "2026-03-02",
    "balance": None, "balanceDate": None, "problems": [3, 4, 5], "skipped": [], "ambiguous": False,
    "dayFirstLine": 3}]})

# 21. zero amounts.
zr = [["11/01/2026", "AUTH HOLD", "0.00"], ["11/01/2026", "COFFEE", "-3.75"], ["11/02/2026", "VOID", "-0.00"],
      ["11/02/2026", "DEPOSIT", "100.00"], ["11/03/2026", "ZERO", "0"]]
write("zero_amounts.csv", csv_bytes([["Date", "Description", "Amount"]] + zr))
expect_csv("zero_amounts.csv", zr, amt3, "2026-11-01", "2026-11-02")

# Preamble lines before the header (some banks add account details first).
pre_rows = [["11/01/2026", "COFFEE", "-3.75"], ["11/02/2026", "DEPOSIT", "100.00"], ["11/03/2026", "FEE", "-1.00"]]
pre_text = ("Account Number : XXXXXX1234\r\nStatement period 11/01/2026 to 11/03/2026\r\n\r\n"
            "Date;Description;Amount\r\n" + "".join(";".join(r) + "\r\n" for r in pre_rows))
write("preamble.csv", pre_text.encode("ascii"))
EXPECTED["files"].append({"file": "preamble.csv", "format": "Csv", "sections": [{
    "kind": "Unknown", "mask": None, "rows": 3, "sum": "95.25", "start": "2026-11-01", "end": "2026-11-03",
    "balance": None, "balanceDate": None, "problems": [], "skipped": [], "rowLines": [5, 6, 7]}]})

# ---------------------------------------------------------------- FITID files
reuse_txns = [dict(fitid="R100", date="20261104", amt="-6.40", type="POS", name="SANDWICH SHOP"),
              dict(fitid="R200", date="20261105", amt="-4.50", type="POS", name="COFFEE CART"),
              dict(fitid="R200", date="20261105", amt="-4.50", type="POS", name="COFFEE CART"),
              dict(fitid="R300", date="20261106", amt="-22.00", type="POS", name="CAR WASH")]
def acct_stmt(txns, start, end, acct="4410082208"):
    return dict(bankid="081000032", acctid=acct, start=start, end=end, txns=txns, bal=None, baldate=None)
r22 = acct_stmt(reuse_txns, "20261104", "20261106")
write("reused_fitid.qfx", ofx([r22], qfx=True).encode("ascii"))
expect_ofx("reused_fitid.qfx", "Qfx", [r22], [{"fitids": ["R100", "R200", "R200", "R300"]}], encoding="Utf8")
r22r = acct_stmt(list(reversed(reuse_txns)), "20261104", "20261106")
write("reused_fitid_reversed.qfx", ofx([r22r], qfx=True).encode("ascii"))
expect_ofx("reused_fitid_reversed.qfx", "Qfx", [r22r], encoding="Utf8")
r22b = acct_stmt([reuse_txns[2]], "20261105", "20261105")
write("reused_fitid_second_only.qfx", ofx([r22b], qfx=True).encode("ascii"))
expect_ofx("reused_fitid_second_only.qfx", "Qfx", [r22b], encoding="Utf8")

nov = [dict(fitid=str(i + 1), date=f"202611{10 + i:02d}", amt=a, type="POS", name=n) for i, (a, n) in
       enumerate([("-10.00", "NOV ONE"), ("-20.00", "NOV TWO"), ("-30.00", "NOV THREE")])]
dec = [dict(fitid=str(i + 1), date=f"202612{10 + i:02d}", amt=a, type="POS", name=n) for i, (a, n) in
       enumerate([("-11.00", "DEC ONE"), ("-21.00", "DEC TWO"), ("-31.00", "DEC THREE"), ("-41.00", "DEC FOUR")])]
s_nov = acct_stmt(nov, "20261110", "20261112")
s_dec = acct_stmt(dec, "20261210", "20261213")
write("per_download_fitid_nov.qfx", ofx([s_nov], qfx=True).encode("ascii"))
write("per_download_fitid_dec.qfx", ofx([s_dec], qfx=True).encode("ascii"))
expect_ofx("per_download_fitid_nov.qfx", "Qfx", [s_nov], encoding="Utf8")
expect_ofx("per_download_fitid_dec.qfx", "Qfx", [s_dec], encoding="Utf8")

lake = next(t for t in op_txns if t["name"] == "LAKESIDE POWER COOP AUTOPAY")
shift = dict(lake, date="20261103")
s_shift = acct_stmt([shift], "20261103", "20261103", acct=OP_ACCT)
write("fitid_date_shift.qfx", ofx([s_shift], qfx=True).encode("ascii"))
expect_ofx("fitid_date_shift.qfx", "Qfx", [s_shift], [{"fitids": [lake["fitid"]]}], encoding="Utf8")

# ---------------------------------------------------------------- malformed
def mal_error(name, data, error, line):
    wmal(name, data)
    EXPECTED["malformed"].append({"file": f"malformed/{name}", "error": error, "line": line})

def mal_problems(name, data, lines):
    wmal(name, data)
    EXPECTED["malformed"].append({"file": f"malformed/{name}", "problems": lines})

mal_problems("bad_amount.csv", csv_bytes([["Date", "Description", "Amount"], ["11/01/2026", "OK", "-1.00"],
                                          ["11/02/2026", "BAD", "12.3x"], ["11/03/2026", "OK", "2.00"]]), [3])
too = acct_stmt([dict(fitid="P1", date="20261101", amt="-1.00", type="POS", name="OK"),
                 dict(fitid="P2", date="20261102", amt="12.345", type="POS", name="TOO PRECISE")], "20261101", "20261102")
# Owner answer Q-P25q (2026-11-07): extra decimals round half away from zero
# to the cent and keep the original text; they are not problems.
rnd = acct_stmt([dict(fitid="Q1", date="20261101", amt="-1.00", type="POS", name="OK"),
                 dict(fitid="Q2", date="20261102", amt="12.345", type="CREDIT", name="ROUNDS UP"),
                 dict(fitid="Q3", date="20261103", amt="-2.005", type="POS", name="ROUNDS AWAY FROM ZERO"),
                 dict(fitid="Q4", date="20261104", amt="7.1000", type="CREDIT", name="TRAILING ZEROS")],
                "20261101", "20261104")
write("rounded_amounts.ofx", ofx([rnd]).encode("ascii"))
EXPECTED["files"].append({"file": "rounded_amounts.ofx", "format": "Ofx", "encoding": "Utf8", "sections": [{
    "kind": "Checking", "mask": mask(rnd["acctid"]), "rows": 4, "sum": "16.44",  # -1.00 + 12.35 - 2.01 + 7.10
    "start": "2026-11-01", "end": "2026-11-04", "balance": None, "balanceDate": None, "problems": [], "skipped": [],
    "roundedFrom": {"2": "12.345", "3": "-2.005"}, "amounts": {"2": "12.35", "3": "-2.01", "4": "7.10"}}]})
full = ofx([too]).replace("\r\n", "\n").split("\n")
cut_at = next(i for i, l in enumerate(full) if l.startswith("<STMTTRN>")) + 3
mal_error("truncated.ofx", "\n".join(full[:cut_at]).encode("ascii"), "Truncated",
          next(i + 1 for i, l in enumerate(full) if l.startswith("<STMTTRN>")))
mal_error("doctype.ofx", b'<?xml version="1.0"?>\n<?OFX OFXHEADER="200" VERSION="220"?>\n<!DOCTYPE OFX [\n'
          b'<!ENTITY lol "lol">\n<!ENTITY lol2 "&lol;&lol;&lol;">\n]>\n<OFX>&lol2;</OFX>\n', "Doctype", 3)
mal_error("entity.ofx", b'<?xml version="1.0"?>\n<?OFX OFXHEADER="200" VERSION="220"?>\n<OFX>\n'
          b'<!ENTITY x SYSTEM "file:///etc/passwd">\n</OFX>\n', "Entity", 4)
mal_problems("bad_date.qif", b"!Type:Bank\nD1/5/26\nT-1.00\nPOK\n^\nD13/45/26\nT-2.00\nPBAD\n^\n", [6])
mal_problems("ragged.csv", csv_bytes([["Date", "Description", "Amount"], ["11/01/2026", "OK", "-1.00"],
                                      ["11/02/2026"], ["11/03/2026", "OK", "2.00"]]), [3])
mal_error("investment.qif", b"!Type:Bank\nD1/5/26\nT-1.00\n^\n\n!Type:Invst\nD1/6/26\nNBuy\n^\n", "Investment", 6)
inv = ofx([too]).replace("<BANKMSGSRSV1>", "<INVSTMTMSGSRSV1>\r\n<INVSTMTTRNRS>\r\n<INVSTMTRS>\r\n</INVSTMTRS>\r\n"
                         "</INVSTMTTRNRS>\r\n</INVSTMTMSGSRSV1>\r\n<BANKMSGSRSV1>")
inv_line = inv.split("\r\n").index("<INVSTMTRS>") + 1
mal_error("investment.ofx", inv.encode("ascii"), "Investment", inv_line)
mal_error("empty.csv", b"", "Empty", None)
sev = ofx([too]).replace("<SEVERITY>INFO\r\n</STATUS>\r\n<DTSERVER>",
                         "<SEVERITY>ERROR\r\n<MESSAGE>Account locked\r\n</STATUS>\r\n<DTSERVER>", 1)
sev = sev.replace("<CODE>0\r\n<SEVERITY>ERROR", "<CODE>15500\r\n<SEVERITY>ERROR", 1)
sev_line = sev.split("\r\n").index("<SEVERITY>ERROR") + 1
mal_error("severity.ofx", sev.encode("ascii"), "BankError", sev_line)
mal_error("unclosed_quote.csv", b'Date,Description,Amount\r\n11/01/2026,"OPEN QUOTE,-1.00\r\n11/02/2026,OK,2.00\r\n',
          "Csv", 2)
mal_error("koi8_charset.ofx", ofx([too], charset="KOI8-R").encode("ascii"), "UnsupportedCharset", 6)

with open(os.path.join(OUT, "expected.json"), "w", encoding="utf-8", newline="\n") as f:
    json.dump(EXPECTED, f, indent=2, ensure_ascii=False)
    f.write("\n")
print(len(EXPECTED["files"]), "fixtures,", len(EXPECTED["malformed"]), "malformed")
