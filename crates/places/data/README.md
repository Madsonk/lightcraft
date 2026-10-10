# Places data

`places-cities.bin` is a **derivative of the GeoNames gazetteer**, converted by `crates/places/examples/build_db.rs`
into a compact zlib-compressed binary table (populated places in k-d tree order).

| | |
|---|---|
| Source | `cities1000.zip` (every populated place with at least 1 000 inhabitants or an administrative seat), `countryInfo.txt`, `admin1CodesASCII.txt` from <https://download.geonames.org/export/dump/> |
| Retrieved | 2026-10-09 |
| Licence | **Creative Commons Attribution 4.0 International (CC BY 4.0)**: <https://creativecommons.org/licenses/by/4.0/> (`COPYING.CC_BY_4.0` in this folder) |
| Content | 171 174 places, 252 countries; per place: name (UTF-8, as GeoNames gives it), latitude and longitude (1e-5 deg), country, first-level division (region), population, quarter flag |
| Changes made | tab-separated text to a binary table; alternate names, ASCII names, elevation, time zones, feature classes other than the quarter flag and all other columns dropped; nothing else (names and numbers unchanged) |
| Size | 2.4 MB |

## What the licence requires

* **Attribution**: keep "Place data from GeoNames (<https://www.geonames.org/>), CC BY 4.0, converted by LightCraft"
  with every copy of the data, and in the app's About / NOTICE. LightCraft does this in `NOTICE` and
  `assets/ATTRIBUTION.md`, and `lightcraft-cli run places.status` prints the provenance of the installed file.
* No ShareAlike: you may adapt and redistribute the data as long as the attribution and a link to the licence stay
  and changes are indicated (the *Changes made* row above is that indication).
* Keep this file separate from code: it is loaded at run time from its own file, never compiled into a binary.

To rebuild: unzip `cities1000.zip`, then
`cargo run --release -p lightcraft-places --example build_db -- cities1000.txt countryInfo.txt admin1CodesASCII.txt places-cities.bin <date> 1000`.
Bigger or smaller dumps (`cities5000`, `cities15000`) work the same way; pass their minimum population as last argument.
