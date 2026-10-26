import datetime
import pathlib
import re

today = datetime.date.today()
text = pathlib.Path("deny.toml").read_text()
stale = []
for match in re.finditer(r'id = "(RUSTSEC-[^"]+)".*?by (\d{4}-\d{2}-\d{2})', text, re.S):
    ident, stamp = match.group(1), match.group(2)
    if datetime.date.fromisoformat(stamp) < today:
        stale.append(ident)
if stale:
    raise SystemExit("past the recheck date: " + ", ".join(stale))
