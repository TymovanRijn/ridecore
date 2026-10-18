#!/usr/bin/env python3
"""Builds the real landscape around a RideCore route from OpenStreetMap.

For a route in assets/routes/<id>.json this asks the Overpass API for land
use, woods, water, rivers, buildings, side roads and village names along
the road, and writes assets/maps/<id>.json with:

- a land cover raster (forest, fields, meadow, town, water, rock, ...) over
  the route's whole mapped area, run-length encoded;
- building footprints near the road (with their number of floors);
- side roads and tracks near the road;
- villages and towns with their names.

Everything is in the route's local meters (x east, z south of its origin),
the same projection as src/route.rs. Overpass answers are cached in
tools/cache/ so reruns are cheap.

Data: © OpenStreetMap contributors (ODbL).

Usage: python3 tools/fetch_map.py <route-id> [...]
"""

import hashlib
import json
import math
import os
import sys
import time
import urllib.parse
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CACHE = os.path.join(ROOT, "tools", "cache")
ROUTES = os.path.join(ROOT, "assets", "routes")
OUT = os.path.join(ROOT, "assets", "maps")
UA = "RideCore-map-builder/0.1 (personal training app)"
OVERPASS = "https://overpass-api.de/api/interpreter"

M_PER_DEG_LAT = 110_540.0
M_PER_DEG_LON = 111_320.0

CELL_M = 20.0
COVER_REACH_M = 2200
BUILDING_REACH_M = 450
ROAD_REACH_M = 700

# Land cover codes, shared with src/route.rs (Cover).
UNKNOWN, FOREST, FARMLAND, MEADOW, TOWN, INDUSTRY, WATER, SCRUB, ROCK, ORCHARD = range(10)
# When areas overlap, the higher priority wins.
PRIORITY = {UNKNOWN: 0, MEADOW: 1, FARMLAND: 2, ORCHARD: 3, SCRUB: 4, FOREST: 5, ROCK: 6, TOWN: 7, INDUSTRY: 8, WATER: 9}


def cover_of(tags):
    lu, nat, lei = tags.get("landuse"), tags.get("natural"), tags.get("leisure")
    if nat == "water" or lu in ("reservoir", "basin") or tags.get("waterway") == "riverbank":
        return WATER
    if lu == "forest" or nat == "wood":
        return FOREST
    if lu == "farmland":
        return FARMLAND
    if lu in ("meadow", "grass", "village_green", "recreation_ground", "allotments") or nat in ("grassland", "heath") or lei in ("park", "golf_course", "pitch", "garden"):
        return MEADOW
    if lu in ("residential", "retail", "commercial", "construction", "cemetery") :
        return TOWN
    if lu in ("industrial", "railway", "brownfield", "landfill", "military"):
        return INDUSTRY
    if nat == "scrub":
        return SCRUB
    if lu == "quarry" or nat in ("bare_rock", "scree", "cliff", "rock", "sand"):
        return ROCK
    if lu in ("orchard", "vineyard", "plant_nursery"):
        return ORCHARD
    return None


def overpass(query, must_find=False):
    """Runs an Overpass query (cached). With `must_find`, an empty answer
    counts as a failure: a busy server sometimes returns nothing at all."""
    key = "osm_" + hashlib.sha1(query.encode()).hexdigest()
    path = os.path.join(CACHE, key + ".json")
    if os.path.exists(path):
        with open(path) as f:
            return json.load(f)
    data = urllib.parse.urlencode({"data": query}).encode()
    for attempt in range(6):
        try:
            req = urllib.request.Request(OVERPASS, data=data, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=300) as r:
                out = json.load(r)
            if must_find and not out.get("elements"):
                raise RuntimeError("empty answer")
            break
        except Exception as e:
            wait = 20 * (attempt + 1)
            print(f"  overpass retry in {wait}s after {e}", file=sys.stderr)
            time.sleep(wait)
    else:
        raise RuntimeError("overpass gave up")
    with open(path, "w") as f:
        json.dump(out, f)
    time.sleep(2)
    return out


def haversine(a, b):
    r = 6371008.8
    la1, lo1, la2, lo2 = map(math.radians, (a[0], a[1], b[0], b[1]))
    h = math.sin((la2 - la1) / 2) ** 2 + math.cos(la1) * math.cos(la2) * math.sin((lo2 - lo1) / 2) ** 2
    return 2 * r * math.asin(math.sqrt(h))


def thin(points, every_m):
    out = [points[0]]
    for p in points[1:]:
        if haversine(out[-1], p) >= every_m:
            out.append(p)
    if out[-1] != points[-1]:
        out.append(points[-1])
    return out


def around(points, radius):
    coords = ",".join(f"{lat:.5f},{lon:.5f}" for lat, lon in points)
    return f"(around:{radius},{coords})"


def join_rings(ways):
    """Joins open way segments (multipolygon members) into closed rings."""
    rings, open_ = [], [list(w) for w in ways if len(w) >= 2]
    while open_:
        ring = open_.pop()
        changed = True
        while ring[0] != ring[-1] and changed:
            changed = False
            for k, w in enumerate(open_):
                if w[0] == ring[-1]:
                    ring += w[1:]
                elif w[-1] == ring[-1]:
                    ring += w[-2::-1]
                elif w[-1] == ring[0]:
                    ring = w[:-1] + ring
                elif w[0] == ring[0]:
                    ring = w[:0:-1] + ring
                else:
                    continue
                open_.pop(k)
                changed = True
                break
        if len(ring) >= 4:
            rings.append(ring)
    return rings


class Raster:
    def __init__(self, x0, z0, nx, nz):
        self.x0, self.z0, self.nx, self.nz = x0, z0, nx, nz
        self.cells = bytearray(nx * nz)

    def put(self, i, j, code):
        if 0 <= i < self.nx and 0 <= j < self.nz:
            k = j * self.nx + i
            if PRIORITY[code] >= PRIORITY[self.cells[k]]:
                self.cells[k] = code

    def fill_polygon(self, ring, code, holes=()):
        """Scanline fill (even-odd, so holes cut out when passed along)."""
        polys = [ring] + list(holes)
        zs = [p[1] for p in ring]
        j0 = max(0, int((min(zs) - self.z0) / CELL_M))
        j1 = min(self.nz - 1, int((max(zs) - self.z0) / CELL_M) + 1)
        for j in range(j0, j1 + 1):
            zc = self.z0 + (j + 0.5) * CELL_M
            xs = []
            for poly in polys:
                for (ax, az), (bx, bz) in zip(poly, poly[1:] + poly[:1]):
                    if (az <= zc < bz) or (bz <= zc < az):
                        xs.append(ax + (zc - az) / (bz - az) * (bx - ax))
            xs.sort()
            for a, b in zip(xs[0::2], xs[1::2]):
                i0 = max(0, int(math.ceil((a - self.x0) / CELL_M - 0.5)))
                i1 = min(self.nx - 1, int(math.floor((b - self.x0) / CELL_M - 0.5)))
                for i in range(i0, i1 + 1):
                    self.put(i, j, code)

    def stroke(self, line, width, code):
        r = width / 2 + CELL_M * 0.35
        for (ax, az), (bx, bz) in zip(line, line[1:]):
            seg = math.hypot(bx - ax, bz - az)
            steps = max(1, int(seg / (CELL_M * 0.5)))
            for s in range(steps + 1):
                t = s / steps
                x, z = ax + (bx - ax) * t, az + (bz - az) * t
                ci, cj = (x - self.x0) / CELL_M, (z - self.z0) / CELL_M
                k = int(r / CELL_M) + 1
                for i in range(int(ci) - k, int(ci) + k + 1):
                    for j in range(int(cj) - k, int(cj) + k + 1):
                        cx, cz = self.x0 + (i + 0.5) * CELL_M, self.z0 + (j + 0.5) * CELL_M
                        if (cx - x) ** 2 + (cz - z) ** 2 <= r * r:
                            self.put(i, j, code)

    def rle(self):
        out, cur, n = [], self.cells[0], 0
        for c in self.cells:
            if c == cur:
                n += 1
            else:
                out += [cur, n]
                cur, n = c, 1
        out += [cur, n]
        return out


RIVER_WIDTH = {"river": 28.0, "canal": 18.0}
ROAD_CLASS = {
    "motorway": (0, 14.0), "trunk": (0, 11.0), "primary": (1, 8.0), "secondary": (1, 7.0),
    "tertiary": (2, 6.0), "unclassified": (2, 5.0), "residential": (2, 5.0), "living_street": (2, 4.5),
    "service": (3, 3.5), "track": (4, 3.0),
}


def build(route_id):
    with open(os.path.join(ROUTES, route_id + ".json")) as f:
        r = json.load(f)
    lat0, lon0 = r["origin"]
    cos0 = math.cos(math.radians(lat0))

    def project(lat, lon):
        return [round((lon - lon0) * cos0 * M_PER_DEG_LON, 1), round((lat0 - lat) * M_PER_DEG_LAT, 1)]

    pts = [(p[0], p[1]) for p in r["points"]]
    line = thin(pts, 250.0)
    print(f"{route_id}: {len(pts)} points, query polyline of {len(line)}")

    a = r["area"]
    x0, z0 = a["i0"] * a["spacing_m"], a["j0"] * a["spacing_m"]
    w, h = (a["ni"] - 1) * a["spacing_m"], (a["nj"] - 1) * a["spacing_m"]
    raster = Raster(x0, z0, int(w / CELL_M), int(h / CELL_M))

    q_cover = f"""[out:json][timeout:300];
(
  way["landuse"]{around(line, COVER_REACH_M)};
  relation["landuse"]{around(line, COVER_REACH_M)};
  way["natural"~"^(wood|water|scrub|grassland|heath|bare_rock|scree|cliff|sand)$"]{around(line, COVER_REACH_M)};
  relation["natural"~"^(wood|water|scrub|grassland|heath)$"]{around(line, COVER_REACH_M)};
  way["leisure"~"^(park|golf_course|pitch|garden)$"]{around(line, COVER_REACH_M)};
  way["waterway"~"^(river|canal|riverbank)$"]{around(line, COVER_REACH_M)};
  relation["waterway"="river"]{around(line, COVER_REACH_M)};
);
out geom;"""
    cover = overpass(q_cover, must_find=True)
    areas, rivers = [], []
    for el in cover["elements"]:
        tags = el.get("tags", {})
        if el["type"] == "way":
            geom = [project(p["lat"], p["lon"]) for p in el.get("geometry", [])]
            if tags.get("waterway") in RIVER_WIDTH:
                width = float(tags.get("width", "0").split()[0] or 0) if tags.get("width", "").replace(".", "").isdigit() else RIVER_WIDTH[tags["waterway"]]
                rivers.append((geom, width))
                continue
            code = cover_of(tags)
            if code is not None and len(geom) >= 4 and geom[0] == geom[-1]:
                areas.append((code, geom[:-1], []))
        elif el["type"] == "relation":
            code = cover_of(tags)
            if tags.get("waterway") == "river":
                for m in el.get("members", []):
                    if m.get("type") == "way" and m.get("role") in ("main_stream", "", "side_stream"):
                        geom = [project(p["lat"], p["lon"]) for p in m.get("geometry", [])]
                        if len(geom) >= 2:
                            rivers.append((geom, RIVER_WIDTH["river"]))
                continue
            if code is None:
                continue
            outer = join_rings([[tuple(project(p["lat"], p["lon"])) for p in m.get("geometry", [])] for m in el.get("members", []) if m.get("role") == "outer" and m.get("type") == "way"])
            inner = join_rings([[tuple(project(p["lat"], p["lon"])) for p in m.get("geometry", [])] for m in el.get("members", []) if m.get("role") == "inner" and m.get("type") == "way"])
            for ring in outer:
                areas.append((code, [list(p) for p in ring[:-1]], [[list(p) for p in hole[:-1]] for hole in inner]))
    # Big, general areas first; small specific ones on top.
    areas.sort(key=lambda a: -polygon_area(a[1]))
    for code, ring, holes in areas:
        raster.fill_polygon(ring, code, holes)
    for geom, width in rivers:
        raster.stroke(geom, width, WATER)
    print(f"  {len(areas)} land cover areas, {len(rivers)} river lines")

    q_near = f"""[out:json][timeout:300];
(
  way["building"]{around(line, BUILDING_REACH_M)};
  way["highway"~"^(motorway|trunk|primary|secondary|tertiary|unclassified|residential|living_street|service|track)$"]{around(line, ROAD_REACH_M)};
  node["place"~"^(city|town|village|hamlet|suburb)$"]{around(line, 3000)};
  node["natural"="peak"]{around(line, 3000)};
);
out geom;"""
    near = overpass(q_near)
    buildings, roads, places = [], [], []
    for el in near["elements"]:
        tags = el.get("tags", {})
        if el["type"] == "node":
            name = tags.get("name")
            if name:
                xz = project(el["lat"], el["lon"])
                kind = tags.get("place") or tags.get("natural")
                places.append([name, xz[0], xz[1], kind])
            continue
        geom = [project(p["lat"], p["lon"]) for p in el.get("geometry", [])]
        if "building" in tags and len(geom) >= 4:
            levels = tags.get("building:levels", "")
            levels = int(float(levels)) if levels.replace(".", "").isdigit() else 0
            kind = tags["building"]
            k = 1 if kind in ("church", "chapel", "cathedral") else 2 if kind in ("industrial", "warehouse", "barn", "farm_auxiliary", "shed", "garage", "garages", "retail", "commercial") else 0
            ring = geom[:-1] if geom[0] == geom[-1] else geom
            buildings.append([levels, k] + [c for p in ring for c in p])
        elif "highway" in tags and len(geom) >= 2:
            cls, width = ROAD_CLASS[tags["highway"]]
            roads.append([cls, width] + [c for p in geom for c in p])
    print(f"  {len(buildings)} buildings, {len(roads)} roads, {len(places)} places")

    out = {
        "id": route_id,
        "attribution": "© OpenStreetMap contributors (ODbL)",
        "cell_m": CELL_M,
        "x0": raster.x0,
        "z0": raster.z0,
        "nx": raster.nx,
        "nz": raster.nz,
        "cover": raster.rle(),
        "buildings": buildings,
        "roads": roads,
        "places": places,
    }
    os.makedirs(OUT, exist_ok=True)
    path = os.path.join(OUT, route_id + ".json")
    with open(path, "w") as f:
        json.dump(out, f, separators=(",", ":"))
    counts = [0] * 10
    for c in raster.cells:
        counts[c] += 1
    total = len(raster.cells)
    names = ["unknown", "forest", "farmland", "meadow", "town", "industry", "water", "scrub", "rock", "orchard"]
    print("  cover: " + ", ".join(f"{n} {100 * c / total:.0f}%" for n, c in zip(names, counts) if c))
    print(f"  wrote {path} ({os.path.getsize(path) // 1024} KB)")


def polygon_area(ring):
    s = 0.0
    for (ax, az), (bx, bz) in zip(ring, ring[1:] + ring[:1]):
        s += ax * bz - bx * az
    return abs(s) / 2


if __name__ == "__main__":
    os.makedirs(CACHE, exist_ok=True)
    for rid in sys.argv[1:] or ["lbl-finale"]:
        build(rid)
