#!/usr/bin/env python3
"""
Scrapes DNV annual Alternative Fuels Insight (AFI) reports for vessel fleet numbers.
Uses agent-browser (batch mode) for JavaScript-rendered DNV news pages.
Writes structured data to data/fleet_demand.json.

Run via cron: 0 6 * * 1  (every Monday at 6am)
"""
import json
import re
import subprocess
from datetime import date
from pathlib import Path

DATA_DIR = Path(__file__).parent.parent / "data"
OUTPUT = DATA_DIR / "fleet_demand.json"
AGENT_BROWSER = "/home/jons-openclaw/.npm-global/bin/agent-browser"

# DNV news search URL for AFI articles.
# Uses DNV's own search to surface the annual AFI orderbook articles.
SEARCH_URL = "https://www.dnv.com/news/?q=alternative+fuel+insight+orderbook"

# AFI article URL keywords (annual report published each January)
AFI_ARTICLE_RE = re.compile(
    r"alternative[_-]fuel|orderbook|order[_-]book|vessel[_-]order|afi|fuelled[_-]vessel",
    re.IGNORECASE,
)

BASELINE_ENTRIES = [
    {"fuel": "Methanol", "ordered_vessels": 323, "delivered_vessels": 112, "avg_consumption_mt": 9500, "color": "#5DADE2"},
    {"fuel": "Biofuel", "ordered_vessels": 20, "delivered_vessels": 11, "avg_consumption_mt": 6800, "color": "#4CAF50"},
    {"fuel": "Ammonia", "ordered_vessels": 45, "delivered_vessels": 2, "avg_consumption_mt": 12000, "color": "#9C27B0"},
    {"fuel": "LNG", "ordered_vessels": 1010, "delivered_vessels": 632, "avg_consumption_mt": 7500, "color": "#FF9800"},
]

PATTERNS = {
    "methanol_cumulative": [
        r"(\d[\d,]+)\s+methanol.fuelled vessels.*?(?:have been ordered|on order)",
    ],
    "methanol_annual": [
        r"[Mm]ethanol orders?.*?(?:fell to|rose to|reached|stood at)\s+(\d[\d,]+)",
        r"[Mm]ethanol.*?accounting for\s+(\d[\d,]+)\s+orders",
    ],
    "lng_cumulative": [
        r"(\d[\d,]+)\s+LNG.fuelled vessels.*?(?:have been ordered|on order)",
    ],
    "lng_annual": [
        r"[Ll][Nn][Gg].*?accounting for\s+(\d[\d,]+)\s+orders",
        r"[Ll][Nn][Gg].fuelled.*?(\d[\d,]+)\s+orders",
    ],
    "ammonia_annual": [
        r"[Aa]mmonia.*?(\d[\d,]+)\s+(?:orders?|vessel|ship)",
    ],
}


def run_batch(commands: list, timeout: int = 60) -> str:
    """Execute agent-browser batch commands, return stdout."""
    try:
        proc = subprocess.Popen(
            [AGENT_BROWSER, "batch"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        stdout, _ = proc.communicate(input=json.dumps(commands), timeout=timeout)
        return stdout
    except (subprocess.TimeoutExpired, FileNotFoundError, OSError) as exc:
        print(f"  batch error: {exc}")
        return ""


def extract_last_value(raw: str) -> str:
    """Pull the last non-empty eval output line from agent-browser stdout."""
    for line in reversed(raw.splitlines()):
        line = line.strip()
        # Skip ANSI color codes and status lines
        if line and not line.startswith("\x1b") and not line.startswith("https://"):
            # Strip surrounding quotes if JSON string
            if line.startswith('"') and line.endswith('"'):
                try:
                    return json.loads(line)
                except json.JSONDecodeError:
                    pass
            return line
    return ""


def get_afi_article_url() -> str | None:
    """Search DNV news and return the first AFI orderbook article URL."""
    print(f"  Searching: {SEARCH_URL}")
    # Use single-quoted JS to avoid JSON escaping issues
    js = "Array.from(document.querySelectorAll('a[href]')).filter(a=>/\\/news\\/\\d{4}\\/.+/.test(a.href)).map(a=>a.href).slice(0,40).join('|')"
    raw = run_batch([
        ["open", SEARCH_URL],
        ["wait", "--load", "networkidle"],
        ["eval", js],
    ])
    links_str = extract_last_value(raw)
    if not links_str:
        return None
    for url in links_str.split("|"):
        if AFI_ARTICLE_RE.search(url):
            return url
    return None


def get_page_body(url: str) -> str:
    """Return the full innerText of a page."""
    print(f"  Fetching: {url}")
    raw = run_batch([
        ["open", url],
        ["wait", "--load", "networkidle"],
        ["eval", "document.body.innerText"],
    ])
    return extract_last_value(raw)


def extract_int(text: str, patterns: list) -> int | None:
    for pat in patterns:
        m = re.search(pat, text, re.IGNORECASE)
        if m:
            return int(m.group(1).replace(",", ""))
    return None


def scrape():
    print(f"[{date.today()}] Starting fleet demand scrape (agent-browser)...")

    try:
        result = subprocess.run([AGENT_BROWSER, "--version"], capture_output=True, text=True, timeout=10)
        if result.returncode != 0:
            raise RuntimeError("non-zero exit")
        print(f"  {result.stdout.strip()}")
    except Exception as exc:
        print(f"  agent-browser unavailable: {exc}, keeping existing data.")
        return

    article_url = get_afi_article_url()
    if not article_url:
        print("  No AFI article found, keeping existing data.")
        return

    print(f"  Found: {article_url}")
    article_text = get_page_body(article_url)
    if not article_text:
        print("  Could not fetch article body, keeping existing data.")
        return

    meoh_cumul = extract_int(article_text, PATTERNS["methanol_cumulative"])
    meoh_annual = extract_int(article_text, PATTERNS["methanol_annual"])
    lng_cumul = extract_int(article_text, PATTERNS["lng_cumulative"])
    lng_annual = extract_int(article_text, PATTERNS["lng_annual"])
    nh3_annual = extract_int(article_text, PATTERNS["ammonia_annual"])

    print(
        f"  MeOH: cumul={meoh_cumul} annual={meoh_annual} | "
        f"LNG: cumul={lng_cumul} annual={lng_annual} | NH3={nh3_annual}"
    )

    if OUTPUT.exists():
        existing = json.loads(OUTPUT.read_text())
        entries = {e["fuel"]: dict(e) for e in existing["entries"]}
        existing_sources = existing.get("sources", [])
    else:
        entries = {e["fuel"]: dict(e) for e in BASELINE_ENTRIES}
        existing_sources = []

    updated = False

    if meoh_cumul and meoh_cumul > 50:
        entries["Methanol"]["ordered_vessels"] = meoh_cumul
        updated = True
    elif meoh_annual and meoh_annual > 10:
        floor = entries["Methanol"]["delivered_vessels"] + meoh_annual
        if floor > entries["Methanol"]["ordered_vessels"]:
            entries["Methanol"]["ordered_vessels"] = floor
            updated = True

    if lng_cumul and lng_cumul > 100:
        entries["LNG"]["ordered_vessels"] = lng_cumul
        updated = True
    elif lng_annual and lng_annual > 50:
        floor = entries["LNG"]["delivered_vessels"] + lng_annual
        if floor > entries["LNG"]["ordered_vessels"]:
            entries["LNG"]["ordered_vessels"] = floor
            updated = True

    if nh3_annual and nh3_annual > 0:
        if nh3_annual > entries["Ammonia"]["ordered_vessels"]:
            entries["Ammonia"]["ordered_vessels"] = nh3_annual
            updated = True

    sources = [s for s in existing_sources if "DNV AFI" not in s]
    sources.insert(0, f"DNV AFI annual report ({article_url})")
    sources = sources[:4]

    result = {
        "entries": list(entries.values()),
        "last_updated": str(date.today()),
        "sources": sources,
    }

    DATA_DIR.mkdir(exist_ok=True)
    OUTPUT.write_text(json.dumps(result, indent=2))
    print(f"  Written to {OUTPUT} (numbers_updated={updated})")


if __name__ == "__main__":
    scrape()
