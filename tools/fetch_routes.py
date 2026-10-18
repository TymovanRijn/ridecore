#!/usr/bin/env python3
"""Builds RideCore's real-world routes.

For every route: geocode named waypoints (Nominatim), route over real roads
with a bicycle profile (OSRM on routing.openstreetmap.de), resample every
20 m, take road elevation from EU-DEM 25 m (OpenTopoData), and sample the
surrounding terrain in a corridor around the road plus a coarse regional
grid (Terrarium tiles, AWS Open Data). Writes assets/routes/<id>.json. All lookups are cached in
tools/cache/ so reruns are cheap and polite to the free services.

Data: © OpenStreetMap contributors (ODbL); EU-DEM v1.1 © European Union,
Copernicus; terrain tiles by Mapzen/Tilezen on AWS Open Data (SRTM, EU-DEM
and other public sources).

Needs Pillow for the terrain tiles: pip install pillow

Usage: python3 tools/fetch_routes.py [route-id ...]
       python3 tools/fetch_routes.py --horizon <route-id ...>   (add far mountains)
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
OUT = os.path.join(ROOT, "assets", "routes")
UA = "RideCore-route-builder/0.1 (personal training app)"

STEP_M = 20.0
TERRAIN_SPACING_M = 60.0
TERRAIN_REACH_M = 420.0
REGION_SPACING_M = 200.0
REGION_MARGIN_M = 2000.0
# Optional horizon grid (--horizon): the real mountains out to this far.
HORIZON_SPACING_M = 500.0
HORIZON_REACH_M = 22_000.0
# Routes that follow cycle paths or cobbled lanes use the bicycle network;
# everything else uses the main road network so climbs stay on the real
# road instead of shortcutting over footpaths.
BIKE_PROFILE = {"afsluitdijk", "kust-noordwijk", "ronde-finale", "box-hill"}

# id, name, region, waypoints, reference km, reference gain m, known
# steepest gradient in % (DEM spikes beyond it are smoothed out).
# A waypoint is "name|cc" (geocoded) or (lat, lon).
ROUTES = [
    ("alpe-dhuez", "Alpe d'Huez", "Isère, Frankrijk", ["Le Bourg-d'Oisans|fr", "Avenue du Rif Nel, Huez|fr"], 13.8, 1100, 13),
    ("ventoux-bedoin", "Mont Ventoux (Bédoin)", "Vaucluse, Frankrijk", ["Bédoin|fr", "Chalet Reynard|fr", "Mont Ventoux|fr"], 21.5, 1600, 12),
    ("galibier", "Col du Galibier (Valloire)", "Savoie, Frankrijk", ["Valloire|fr", "Plan Lachat|fr", "Col du Galibier|fr"], 18.1, 1245, 12),
    ("tourmalet", "Col du Tourmalet (Luz)", "Hautes-Pyrénées, Frankrijk", ["Luz-Saint-Sauveur|fr", "Barèges|fr", "Col du Tourmalet|fr"], 19.0, 1400, 12),
    ("stelvio", "Passo dello Stelvio (Prato)", "Zuid-Tirol, Italië", ["Prato allo Stelvio|it", "Trafoi|it", "Passo dello Stelvio|it"], 24.3, 1808, 12),
    ("gavia", "Passo di Gavia (Ponte di Legno)", "Lombardije, Italië", ["Ponte di Legno|it", "Passo di Gavia|it"], 17.3, 1363, 16),
    ("mortirolo", "Passo del Mortirolo (Mazzo)", "Lombardije, Italië", ["Mazzo di Valtellina|it", "Passo del Mortirolo|it"], 12.4, 1300, 18),
    ("giau", "Passo Giau (Selva di Cadore)", "Dolomieten, Italië", ["Selva di Cadore|it", "Passo Giau|it"], 10.0, 922, 14),
    ("zoncolan", "Monte Zoncolan (Ovaro)", "Friuli, Italië", ["Ovaro|it", "Liariis|it", "Monte Zoncolan|it"], 10.1, 1210, 22),
    ("pordoi", "Passo Pordoi (Canazei)", "Dolomieten, Italië", ["Canazei|it", "Passo Pordoi|it"], 12.0, 780, 10),
    ("sa-calobra", "Sa Calobra", "Mallorca, Spanje", ["Sa Calobra|es", "Coll dels Reis|es"], 9.9, 680, 12),
    ("puig-major", "Puig Major (Sóller)", "Mallorca, Spanje", ["Sóller|es", "Coll de Puig Major|es"], 14.0, 800, 9),
    ("angliru", "Alto de l'Angliru", "Asturië, Spanje", ["La Riera, Riosa|es", "Alto del Angliru|es"], 12.5, 1266, 24),
    ("covadonga", "Lagos de Covadonga", "Asturië, Spanje", ["Cangas de Onís|es", "Covadonga|es", "Lago Enol|es"], 24.0, 1100, 16),
    ("madeleine", "Col de la Madeleine (La Chambre)", "Savoie, Frankrijk", ["La Chambre, Savoie|fr", "Col de la Madeleine|fr"], 19.2, 1520, 12),
    ("izoard", "Col d'Izoard (Briançon)", "Hautes-Alpes, Frankrijk", ["Briançon|fr", "Cervières, Hautes-Alpes|fr", "Col d'Izoard|fr"], 20.0, 1100, 12),
    ("croix-de-fer", "Col de la Croix de Fer", "Savoie, Frankrijk", ["Saint-Jean-de-Maurienne|fr", "Saint-Sorlin-d'Arves|fr", "Col de la Croix de Fer|fr"], 29.0, 1520, 12),
    ("grand-colombier", "Grand Colombier (Culoz)", "Ain, Frankrijk", ["Culoz|fr", "Col du Grand Colombier|fr"], 18.3, 1255, 14),
    ("hautacam", "Hautacam", "Hautes-Pyrénées, Frankrijk", ["Argelès-Gazost|fr", "Hautacam|fr"], 15.6, 1100, 13),
    ("ballon-dalsace", "Ballon d'Alsace (Giromagny)", "Vogezen, Frankrijk", ["Giromagny|fr", "Ballon d'Alsace|fr"], 13.0, 790, 9),
    ("grossglockner", "Großglockner Hochalpenstraße", "Salzburg, Oostenrijk", ["Fusch an der Großglocknerstraße|at", "Fuscher Törl|at", "Hochtor|at"], 30.0, 1900, 13),
    ("kitzbueheler-horn", "Kitzbüheler Horn", "Tirol, Oostenrijk", ["Kitzbühel|at", "Kitzbüheler Horn|at"], 9.9, 1230, 22),
    ("ronde-finale", "Ronde van Vlaanderen finale", "Vlaanderen, België", ["Oudenaarde|be", "Oude Kwaremont|be", "Paterberg|be", "Koppenberg|be", "Oudenaarde|be"], 35.0, 400, 22),
    ("lbl-finale", "Luik–Bastenaken–Luik finale", "Wallonië, België", ["Remouchamps|be", "La Redoute|be", "Sprimont|be", "Roche-aux-Faucons|be", "Seraing|be"], 40.0, 650, 14),
    ("amstel-finale", "Amstel Gold Race finale", "Limburg, Nederland", ["Valkenburg|nl", "Keutenberg|nl", "Gulpen|nl", "Cauberg|nl", "Berg en Terblijt|nl"], 30.0, 400, 12),
    ("sanremo-finale", "Milaan–San Remo finale", "Ligurië, Italië", ["Imperia|it", "Cipressa|it", "San Lorenzo al Mare|it", "Poggio, Sanremo|it", "Sanremo|it"], 30.0, 450, 9),
    ("mur-de-bretagne", "Mûr-de-Bretagne", "Bretagne, Frankrijk", ["Saint-Guen|fr", "Mûr-de-Bretagne|fr"], 12.0, 200, 15),
    ("box-hill", "Box Hill (Zig Zag Road)", "Surrey, Verenigd Koninkrijk", ["Dorking|gb", "Burford Bridge|gb", "Box Hill, Surrey|gb", "Headley, Surrey|gb", "Dorking|gb"], 15.0, 250, 12),
    ("afsluitdijk", "Afsluitdijk", "Friesland/Noord-Holland, Nederland", ["Den Oever|nl", "Kornwerderzand|nl", "Zurich, Friesland|nl"], 32.0, 10, 4),
    ("kust-noordwijk", "Kustroute Zandvoort–Noordwijk", "Zuid-Holland, Nederland", ["Zandvoort|nl", "Langevelderslag|nl", "Noordwijk|nl"], 17.0, 20, 6),
]


def http_json(url, data=None, cache_key=None, pause=1.1):
    if cache_key:
        path = os.path.join(CACHE, cache_key + ".json")
        if os.path.exists(path):
            with open(path) as f:
                return json.load(f)
    for attempt in range(8):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60) as r:
                out = json.load(r)
            time.sleep(pause)
            break
        except Exception as e:  # rate limits and flaky networks: back off
            wait = (30 if "429" in str(e) else 5) * (attempt + 1)
            print(f"  retry in {wait}s after {e}", file=sys.stderr)
            time.sleep(wait)
    else:
        raise RuntimeError(f"giving up on {url}")
    if cache_key:
        with open(path, "w") as f:
            json.dump(out, f)
    return out


def safe(s):
    return "".join(c if c.isalnum() else "_" for c in s)[:80]


def geocode(wp):
    if isinstance(wp, tuple):
        return wp
    name, cc = wp.split("|")
    q = urllib.parse.urlencode({"q": name, "countrycodes": cc, "format": "json", "limit": 1})
    res = http_json(f"https://nominatim.openstreetmap.org/search?{q}", cache_key="geo_" + safe(wp))
    if not res:
        raise RuntimeError(f"not found: {wp}")
    return float(res[0]["lat"]), float(res[0]["lon"])


def route(points, profile):
    coords = ";".join(f"{lon:.6f},{lat:.6f}" for lat, lon in points)
    url = f"https://routing.openstreetmap.de/routed-{profile}/route/v1/driving/{coords}?overview=full&geometries=geojson"
    res = http_json(url, cache_key=f"route_{profile}_" + safe(coords))
    if res.get("code") != "Ok":
        raise RuntimeError(f"routing failed: {res.get('code')}")
    return [(lat, lon) for lon, lat in res["routes"][0]["geometry"]["coordinates"]]


def haversine(a, b):
    r = 6371008.8
    la1, lo1, la2, lo2 = map(math.radians, (a[0], a[1], b[0], b[1]))
    h = math.sin((la2 - la1) / 2) ** 2 + math.cos(la1) * math.cos(la2) * math.sin((lo2 - lo1) / 2) ** 2
    return 2 * r * math.asin(math.sqrt(h))


def resample(line, step):
    out = [line[0]]
    carry = 0.0
    for a, b in zip(line, line[1:]):
        seg = haversine(a, b)
        if seg == 0:
            continue
        pos = step - carry
        while pos <= seg:
            t = pos / seg
            out.append((a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t))
            pos += step
        carry = (carry + seg) % step
    if haversine(out[-1], line[-1]) > 1.0:
        out.append(line[-1])
    return out


def chunked(xs, n):
    for i in range(0, len(xs), n):
        yield xs[i : i + n]


def elevations_eudem(points):
    out = []
    for batch in chunked(points, 100):
        locs = "|".join(f"{lat:.6f},{lon:.6f}" for lat, lon in batch)
        key = "eudem_" + hashlib.sha1(locs.encode()).hexdigest()
        res = http_json(f"https://api.opentopodata.org/v1/eudem25m?locations={locs}", cache_key=key)
        if res.get("status") != "OK":
            raise RuntimeError(f"elevation failed: {res}")
        out.extend(r["elevation"] for r in res["results"])
    # Points over water (coastal roads, dikes) have no DEM value: carry the
    # nearest known height across the gap.
    known = next((e for e in out if e is not None), 0.0)
    for i, e in enumerate(out):
        if e is None:
            out[i] = known
        else:
            known = e
    return out


TERRARIUM_ZOOM = 12
_tiles = {}


def terrarium_tile(x, y):
    """One Terrarium elevation tile (AWS Open Data, Mapzen/Tilezen): RGB
    encodes height as R*256 + G + B/256 - 32768 meters."""
    from PIL import Image  # pip install pillow

    key = (x, y)
    if key not in _tiles:
        path = os.path.join(CACHE, f"terrarium_{TERRARIUM_ZOOM}_{x}_{y}.png")
        if not os.path.exists(path):
            url = f"https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{TERRARIUM_ZOOM}/{x}/{y}.png"
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60) as r, open(path, "wb") as f:
                f.write(r.read())
        _tiles[key] = Image.open(path).convert("RGB").load()
    return _tiles[key]


def terrain_height(lat, lon):
    """Bilinear terrain height from Terrarium tiles."""
    n = 2**TERRARIUM_ZOOM
    fx = (lon + 180.0) / 360.0 * n * 256
    lat_r = math.radians(lat)
    fy = (1.0 - math.asinh(math.tan(lat_r)) / math.pi) / 2.0 * n * 256

    def px(gx, gy):
        tile = terrarium_tile(gx // 256, gy // 256)
        r, g, b = tile[gx % 256, gy % 256]
        return r * 256 + g + b / 256 - 32768

    x0, y0 = int(fx), int(fy)
    tx, ty = fx - x0, fy - y0
    top = px(x0, y0) * (1 - tx) + px(x0 + 1, y0) * tx
    bottom = px(x0, y0 + 1) * (1 - tx) + px(x0 + 1, y0 + 1) * tx
    return top * (1 - ty) + bottom * ty


def elevations_terrain(points):
    return [terrain_height(lat, lon) for lat, lon in points]


def smooth(values, window):
    """Median then mean filter: removes DEM spikes at bridges and cuts while
    keeping the real gradient of the road."""
    n = len(values)
    med = [sorted(values[max(0, i - 3) : i + 4])[len(values[max(0, i - 3) : i + 4]) // 2] for i in range(n)]
    half = window // 2
    return [sum(med[max(0, i - half) : i + half + 1]) / len(med[max(0, i - half) : i + half + 1]) for i in range(n)]


def projector(points):
    lat0 = sum(p[0] for p in points) / len(points)
    lon0 = sum(p[1] for p in points) / len(points)
    k = math.cos(math.radians(lat0))

    def to_xz(lat, lon):
        return (lon - lon0) * k * 111_320.0, (lat0 - lat) * 110_540.0

    def to_latlon(x, z):
        return lat0 - z / 110_540.0, lon0 + x / (k * 111_320.0)

    return (lat0, lon0), to_xz, to_latlon


def terrain(points, to_xz, to_latlon):
    """Terrain heights on a grid of cells within reach of the road."""
    cells = set()
    reach = int(TERRAIN_REACH_M / TERRAIN_SPACING_M) + 1
    for lat, lon in points[::3]:
        x, z = to_xz(lat, lon)
        ci, cj = round(x / TERRAIN_SPACING_M), round(z / TERRAIN_SPACING_M)
        for di in range(-reach, reach + 1):
            for dj in range(-reach, reach + 1):
                if (di * di + dj * dj) * TERRAIN_SPACING_M**2 <= TERRAIN_REACH_M**2:
                    cells.add((ci + di, cj + dj))
    cells = sorted(cells)
    latlons = [to_latlon(i * TERRAIN_SPACING_M, j * TERRAIN_SPACING_M) for i, j in cells]
    heights = elevations_terrain(latlons)
    return [[i, j, round(h, 1)] for (i, j), h in zip(cells, heights)]


def regional(points, to_xz, to_latlon):
    """A coarse height grid over the whole area, for valleys and slopes up to
    the horizon. Stored as a full rectangle, row by row."""
    xs, zs = zip(*(to_xz(lat, lon) for lat, lon in points[::5]))
    i0 = math.floor((min(xs) - REGION_MARGIN_M) / REGION_SPACING_M)
    j0 = math.floor((min(zs) - REGION_MARGIN_M) / REGION_SPACING_M)
    i1 = math.ceil((max(xs) + REGION_MARGIN_M) / REGION_SPACING_M)
    j1 = math.ceil((max(zs) + REGION_MARGIN_M) / REGION_SPACING_M)
    latlons = [to_latlon(i * REGION_SPACING_M, j * REGION_SPACING_M) for j in range(j0, j1 + 1) for i in range(i0, i1 + 1)]
    heights = elevations_terrain(latlons)
    return {"spacing_m": REGION_SPACING_M, "i0": i0, "j0": j0, "ni": i1 - i0 + 1, "nj": j1 - j0 + 1, "heights": [round(h) for h in heights]}


def horizon(rid):
    """Adds a coarse height grid reaching far beyond the route to an existing
    route file, so the real mountains stand on the horizon."""
    path = os.path.join(OUT, rid + ".json")
    with open(path) as f:
        data = json.load(f)
    line = [(lat, lon) for lat, lon, _ in data["points"]]
    _, to_xz, to_latlon = projector(line)
    xs, zs = zip(*(to_xz(lat, lon) for lat, lon in line[::5]))
    cx, cz = (min(xs) + max(xs)) / 2, (min(zs) + max(zs)) / 2
    s = HORIZON_SPACING_M
    i0, j0 = math.floor((cx - HORIZON_REACH_M) / s), math.floor((cz - HORIZON_REACH_M) / s)
    i1, j1 = math.ceil((cx + HORIZON_REACH_M) / s), math.ceil((cz + HORIZON_REACH_M) / s)
    latlons = [to_latlon(i * s, j * s) for j in range(j0, j1 + 1) for i in range(i0, i1 + 1)]
    heights = elevations_terrain(latlons)
    data["horizon"] = {"spacing_m": s, "i0": i0, "j0": j0, "ni": i1 - i0 + 1, "nj": j1 - j0 + 1, "heights": [round(h) for h in heights]}
    with open(path, "w") as f:
        json.dump(data, f, separators=(",", ":"))
    print(f"{rid:22s} horizon {i1 - i0 + 1}x{j1 - j0 + 1} cells, {min(heights):.0f}..{max(heights):.0f} m", flush=True)


def limit_grades(ele, dist, max_pct):
    """Smooths only where the 100 m gradient exceeds the climb's known
    maximum (DEM artifacts on hairpins and cliff roads); the rest is kept."""
    ele = list(ele)
    for _ in range(200):
        bad = [i for i in range(len(ele) - 5) if abs(ele[i + 5] - ele[i]) / max(dist[i + 5] - dist[i], 1.0) * 100 > max_pct + 1.0]
        if not bad:
            break
        for i in bad:
            lo, hi = max(1, i - 6), min(len(ele) - 1, i + 11)
            for k in range(lo, hi):
                ele[k] = (ele[k - 1] + ele[k] + ele[k + 1]) / 3.0
    return ele


def build(rid, name, region, waypoints, ref_km, ref_gain, ref_max):
    pts = [geocode(w) for w in waypoints]
    line = resample(route(pts, "bike" if rid in BIKE_PROFILE else "car"), STEP_M)
    # ~180 m smoothing: DEM cells on hairpins and in cuttings mix in the
    # slope beside the road; the road itself changes grade gradually.
    ele = smooth(elevations_eudem(line), 9)
    dist = [0.0]
    for a, b in zip(line, line[1:]):
        dist.append(dist[-1] + haversine(a, b))
    ele = limit_grades(ele, dist, ref_max)
    gain = sum(max(0.0, b - a) for a, b in zip(ele, ele[1:]))
    looped = haversine(line[0], line[-1]) < 250 and len(line) > 10
    origin, to_xz, to_latlon = projector(line)
    grid = terrain(line, to_xz, to_latlon)
    area = regional(line, to_xz, to_latlon)
    max_grade = max(((ele[i + 10] - ele[i]) / (dist[i + 10] - dist[i]) * 100 for i in range(len(ele) - 10)), default=0)
    out = {
        "id": rid,
        "name": name,
        "region": region,
        "looped": looped,
        "origin": [round(origin[0], 7), round(origin[1], 7)],
        "points": [[round(lat, 6), round(lon, 6), round(e, 1)] for (lat, lon), e in zip(line, ele)],
        "terrain_spacing_m": TERRAIN_SPACING_M,
        "terrain": grid,
        "area": area,
    }
    with open(os.path.join(OUT, rid + ".json"), "w") as f:
        json.dump(out, f, separators=(",", ":"))
    km = dist[-1] / 1000
    flag = "" if abs(km - ref_km) / ref_km < 0.25 and (ref_gain < 50 or abs(gain - ref_gain) / ref_gain < 0.25) else "  <-- CHECK"
    print(f"{rid:22s} {km:6.1f} km (ref {ref_km:5.1f})  +{gain:5.0f} m (ref {ref_gain:5d})  max {max_grade:4.1f}% (ref {ref_max})  loop={looped}  terrain {len(grid)}{flag}", flush=True)


def main():
    os.makedirs(CACHE, exist_ok=True)
    os.makedirs(OUT, exist_ok=True)
    if sys.argv[1:2] == ["--horizon"]:
        for rid in sys.argv[2:]:
            horizon(rid)
        return
    wanted = set(sys.argv[1:])
    for r in ROUTES:
        if wanted and r[0] not in wanted:
            continue
        try:
            build(*r)
        except Exception as e:
            print(f"{r[0]:22s} FAILED: {e}", flush=True)


if __name__ == "__main__":
    main()
