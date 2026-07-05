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
| `docs/tokyo.png` | Rendered by coolviz; building geometry derived from **Project PLATEAU** (国土交通省) 東京23区 3D都市モデル (2020) | CC BY 4.0-compatible; attribution: "Source: 国土交通省 Project PLATEAU" |
| `docs/okinawa.png` | Fully procedural render, no external data | Same terms as the code (MIT) |

Runtime data feeds (not stored in the repo): NOAA NOMADS (public domain),
CelesTrak (per their terms), USGS earthquake feeds (public domain), the
NICT Himawari real-time web (non-commercial), Project PLATEAU 3D Tiles
(CC BY 4.0-compatible, cached under `.cache/plateau/`), and the JMA
high-resolution precipitation nowcast tiles (**unofficial endpoint** — the
same one jma.go.jp itself uses; polled politely, no SLA, may break anytime;
JMA data reuse requires attribution under 気象業務法-related terms). If you
fork this project for anything commercial, disable the Himawari layer or
license imagery appropriately, and re-render the screenshots without it.
