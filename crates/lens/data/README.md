# Lens profile data

`lens-profiles.json.z` is a **derivative of the lensfun lens database** (data files only), converted by
`crates/lens/examples/build_db.rs` into a compact zlib-compressed JSON file.

| | |
|---|---|
| Source | the XML files `data/db/*.xml` of the lensfun project, <https://lensfun.github.io/> (repository `lensfun/lensfun`) |
| Revision | `43f9e001ab66c4fcdd4f400463f13b72f94b2288` (last commit touching `data/db`, 2026-10-08), retrieved 2026-10-09 |
| Licence | **Creative Commons Attribution-ShareAlike 3.0 Unported (CC BY-SA 3.0)**: <https://creativecommons.org/licenses/by-sa/3.0/> (`COPYING.CC_BY-SA_3.0` in this folder) |
| Content | 52 mounts, 1 057 cameras, 1 571 lens entries (1 568 with calibration): distortion (PanoTools `ptlens`, `poly3`, `poly5`), lateral chromatic aberration (`linear`, `poly3`), vignetting (`pa`) |
| Changes made | XML to JSON; short field names; the focal range and widest aperture are filled from the lens name or the calibration's focal lengths when the entry gave none; nothing else (numbers are unchanged) |

## What the licence requires

* **Attribution**: keep "Lens profile data from the lensfun project (<https://lensfun.github.io/>), CC BY-SA 3.0, with
  the changes listed above" with every copy of the data, and in the app's About / NOTICE. LightCraft does this in `NOTICE`
  and `assets/ATTRIBUTION.md`, and `lightcraft-cli run lens.info` prints the source, licence and revision of the
  installed file.
* **ShareAlike**: if you **adapt** this data (convert it again, filter it, add or change entries) and **distribute** the
  result, the adapted data file must be offered under CC BY-SA 3.0 (or a licence Creative Commons lists as compatible).
  Keep this file separate from code: it is loaded at run time from its own file, never compiled into a binary, so the
  rest of LightCraft stays MIT OR Apache-2.0. Do not paste these numbers into source files.
* The CC BY-SA 3.0 licence applies to the **data**. LightCraft reads **only the XML data format**; the lensfun library
  (LGPL) and its code were not read, linked or copied, and no lensfun software is needed or shipped.

This is an engineering reading of the licence, not legal advice: have it validated before a commercial release
(the points to settle are whether a corrections table is copyrightable at all, and how ShareAlike applies when the file
ships next to the application in one package; keeping it a separate, clearly licensed file is the conservative answer).

## Updating

```sh
# fetch the *.xml files of data/db from the lensfun repository into a folder, then
cargo run --release -p lightcraft-lens --example build_db -- <folder> crates/lens/data/lens-profiles.json.z <revision> <date>
```

Contributions of new calibrations belong upstream (lensfun), where everyone benefits and the licence is already right.
