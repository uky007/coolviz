# Licensing & data provenance / ライセンスとデータ出所

**Code** (`src/`, `Cargo.toml`, build files): MIT — see [LICENSE](LICENSE).

The repository also contains **data snapshots and rendered screenshots whose
terms differ from MIT**:

| Path | Origin | Terms |
|---|---|---|
| `assets/ne_50m_coastline.geojson.gz`, `assets/ne_110m_land.geojson.gz` | [Natural Earth](https://www.naturalearthdata.com) | Public domain |
| `assets/wind_snapshot.grib2` | NOAA GFS via [NOMADS](https://nomads.ncep.noaa.gov) | US Government work, public domain |
| `assets/sats_snapshot.json.gz` | [CelesTrak](https://celestrak.org) GP catalog | © CelesTrak; redistributed here solely as an offline-boot snapshot, with attribution. Fetch fresh data from CelesTrak (the app honors their one-download-per-2h policy) |
| `docs/hero.png`, `docs/typhoon.png` | Rendered by coolviz; **includes Himawari-9 imagery via [NICT](https://himawari8.nict.go.jp)** | **Non-commercial use only** (NICT terms). These two images are NOT covered by the MIT license |
| `docs/night.png` | Rendered by coolviz from public-domain sources only | Same terms as the code (MIT) |

Runtime data feeds (not stored in the repo): NOAA NOMADS (public domain),
CelesTrak (per their terms), USGS earthquake feeds (public domain), and the
NICT Himawari real-time web (non-commercial). If you fork this project for
anything commercial, disable the Himawari layer or license imagery
appropriately, and re-render the screenshots without it.
