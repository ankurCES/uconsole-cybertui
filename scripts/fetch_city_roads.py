#!/usr/bin/env python3
"""One-shot Overpass → bundled-city fetcher (Step 6).

Fetches road polylines for a named city from the public Overpass
API and emits a compact JSON file that the TUI bundles via
`include_str!` in `crates/tui/src/screens/city/roads.rs`.

Why a one-shot script?
    Overpass rate-limits anonymous requests (~10/min). Bundling the
    result means users never hit Overpass on launch and the city
    screen works fully offline. The trade-off is that bundled data
    ages — when streets close or new roads open, run this script
    again and commit the regenerated file.

Output shape (matches `roads::CityRoads`):

    {
      "name": "Seattle",
      "bbox": [min_lat, min_lon, max_lat, max_lon],
      "roads": [
        { "importance": "motorway", "points": [[lat, lon], ...] },
        ...
      ]
    }

Usage:
    ./scripts/fetch_city_roads.py seattle
    ./scripts/fetch_city_roads.py london --bbox 51.4,-0.2,51.6,0.1

Each `<slug>.json` lands in `crates/tui/data/cities/<slug>.json`.
Step 7 (the braille renderer) reads these via `include_str!` once
they're committed.

Run dry first:
    ./scripts/fetch_city_roads.py --dry-run seattle
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.parse
import urllib.request
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Iterable

OVERPASS_ENDPOINT = "https://overpass-api.de/api/interpreter"

# Importance buckets the TUI groups + colours differently. The
# Overpass query asks for exactly these highway tags so the JSON is
# bounded; anything outside this set is dropped.
SUPPORTED_TAGS: tuple[str, ...] = (
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "residential",
    "footway",
)


@dataclass(frozen=True)
class Road:
    importance: str
    points: list[list[float]]  # [[lat, lon], ...]


@dataclass(frozen=True)
class CityDocument:
    name: str
    bbox: list[float]
    roads: list[Road]


# City presets. Each preset fixes (center name, default bbox) so a
# bare `./scripts/fetch_city_roads.py seattle` Just Works. The bbox
# is [south, west, north, east] (Overpass / OSM ordering).
CITY_PRESETS: dict[str, tuple[str, tuple[float, float, float, float]]] = {
    # [name, (south, west, north, east)]
    "seattle": ("Seattle", (47.4810, -122.4590, 47.7340, -122.2240)),
    "london": ("London", (51.401, -0.270, 51.580, 0.040)),
    "tokyo": ("Tokyo", (35.500, 139.500, 35.850, 139.950)),
    "berlin": ("Berlin", (52.350, 13.150, 52.650, 13.700)),
    "nyc": ("New York", (40.550, -74.100, 40.900, -73.700)),
}


def overpass_query(south: float, west: float, north: float, east: float) -> str:
    """Overpass QL for every highway way inside the bbox, with
    importance-bucketed tags so the response stays manageable for a
    city-scale query (a few MB max)."""
    bbox = f"{south},{west},{north},{east}"
    selectors = "|".join(f"highway={t}" for t in SUPPORTED_TAGS)
    return f"""
[out:json][timeout:60];
(
  way[{selectors}]({bbox});
);
out body geom;
""".strip()


def http_post_form(url: str, data: dict[str, str]) -> bytes:
    encoded = urllib.parse.urlencode(data).encode()
    req = urllib.request.Request(
        url,
        data=encoded,
        headers={
            "User-Agent": "cyberdeck-tui/0.1 (bundled-city-fetcher)",
            "Accept": "application/json",
        },
    )
    with urllib.request.urlopen(req, timeout=120) as resp:
        return resp.read()


def parse_overpass(payload: bytes, importance_order: dict[str, int]) -> list[Road]:
    """Convert the Overpass JSON into our flat `Road` list. We pick
    one importance per way (the highest-priority tag in the OSM tag
    set) so a way tagged `motorway_link` is rendered alongside the
    primary motorways, not separately."""
    doc = json.loads(payload)
    elements = doc.get("elements", [])
    roads: list[Road] = []
    for el in elements:
        if el.get("type") != "way":
            continue
        tags = el.get("tags", {})
        # Pick the highest-priority importance tag for the way.
        tag: str | None = None
        for candidate in SUPPORTED_TAGS:
            if tags.get("highway") == candidate:
                tag = candidate
                break
        if tag is None:
            continue
        geom = el.get("geometry") or []
        points = [[pt["lat"], pt["lon"]] for pt in geom if "lat" in pt and "lon" in pt]
        if len(points) < 2:
            continue
        roads.append(Road(importance=tag, points=points))
    # Stable output order: importance desc, then by first-point lat.
    roads.sort(
        key=lambda r: (
            -importance_order.get(r.importance, 0),
            r.points[0][0],
        )
    )
    return roads


def importance_order() -> dict[str, int]:
    return {tag: i for i, tag in enumerate(reversed(SUPPORTED_TAGS))}


def derive_bbox(roads: Iterable[Road], fallback: tuple[float, float, float, float]) -> list[float]:
    roads = list(roads)
    if not roads:
        return list(fallback)
    lats: list[float] = []
    lons: list[float] = []
    for r in roads:
        for pt in r.points:
            lats.append(pt[0])
            lons.append(pt[1])
    return [min(lats), min(lons), max(lats), max(lons)]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("slug", help="city slug (e.g. 'seattle'). Used as the JSON filename.")
    parser.add_argument(
        "--bbox",
        help="override bbox as 'south,west,north,east' (decimal degrees)",
    )
    parser.add_argument("--out", help="override output path (default: crates/tui/data/cities/<slug>.json)")
    parser.add_argument("--dry-run", action="store_true", help="print the Overpass query, do not POST")
    parser.add_argument("--overpass-url", default=OVERPASS_ENDPOINT, help="alternate Overpass endpoint (for kumi systems etc.)")
    args = parser.parse_args(argv)

    if args.slug not in CITY_PRESETS and not args.bbox:
        print(f"error: unknown slug {args.slug!r}; pass --bbox south,west,north,east", file=sys.stderr)
        return 2

    name, bbox = CITY_PRESETS.get(args.slug, (args.slug, (0.0, 0.0, 0.0, 0.0)))
    if args.bbox:
        try:
            bbox = tuple(float(x) for x in args.bbox.split(","))  # type: ignore[assignment]
        except ValueError:
            print(f"error: --bbox expected 'south,west,north,east', got {args.bbox!r}", file=sys.stderr)
            return 2

    south, west, north, east = bbox
    query = overpass_query(south, west, north, east)

    if args.dry_run:
        print(query)
        return 0

    print(f"Fetching {name} ({south},{west},{north},{east}) from {args.overpass_url}…", file=sys.stderr)
    started = time.time()
    try:
        payload = http_post_form(args.overpass_url, {"data": query})
    except Exception as exc:  # network / HTTP / timeout
        print(f"error: Overpass request failed: {exc}", file=sys.stderr)
        return 1
    print(f"  got {len(payload):,} bytes in {time.time() - started:.1f}s", file=sys.stderr)

    roads = parse_overpass(payload, importance_order())
    final_bbox = derive_bbox(roads, bbox)
    document = CityDocument(name=name, bbox=final_bbox, roads=roads)

    out_path = Path(
        args.out
        or Path(__file__).resolve().parent.parent / "crates/tui/data/cities" / f"{args.slug}.json"
    )
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(asdict(document), indent=2) + "\n")
    print(f"  wrote {len(roads)} roads → {out_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
