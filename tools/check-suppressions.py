import datetime
import pathlib
import re

today = datetime.date.today()
text = pathlib.Path("deny.toml").read_text()
stale = []
undated = []
identifiers = set()
for match in re.finditer(r'\{\s*id = "(RUSTSEC-[^"]+)"(.*?)\}', text, re.S):
    ident, body = match.group(1), match.group(2)
    identifiers.add(ident)
    stamp = re.search(r"by (\d{4}-\d{2}-\d{2})", body)
    if stamp is None:
        undated.append(ident)
        continue
    if datetime.date.fromisoformat(stamp.group(1)) < today:
        stale.append(ident)
if undated:
    raise SystemExit("no recheck date in the reason: " + ", ".join(undated))
if stale:
    raise SystemExit("past the recheck date: " + ", ".join(stale))

audit_config = pathlib.Path(".cargo/audit.toml")
if audit_config.exists():
    ignore = re.search(r"ignore = \[(.*?)\]", audit_config.read_text(), re.S)
    listed = set(re.findall(r'"(RUSTSEC-[^"]+)"', ignore.group(1))) if ignore else set()
    if listed != identifiers:
        raise SystemExit(
            ".cargo/audit.toml ignores differ from deny.toml; only in audit.toml: "
            + ", ".join(sorted(listed - identifiers))
            + "; only in deny.toml: "
            + ", ".join(sorted(identifiers - listed))
        )
