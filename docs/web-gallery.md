# Web gallery export

`gallery.export` (File ▸ Export Web Gallery…) writes a static web gallery of the selected photos (else
the photos in view) to a folder: nothing to host but files, no external request, no tracking.

```
gallery/
  index.html     the page: responsive <picture>/srcset grid, lazy loading, lightbox (←/→, click to close),
                 JSON-LD ImageGallery for search engines
  snippet.html   the grid alone, with its style and no script: paste it in an Odoo page ("Embed code")
  gallery.json   machine-readable list: per photo name, alt text and where it came from, every file
  img/           plat-480.avif, plat-480.jpg, plat-960.avif … (the photo's name, a slug; the id is
                 added when two photos have the same name)
```

Each photo is rendered once per width (never enlarged: a photo smaller than a rung does not repeat
it) and encoded in each format from that render. Defaults: widths `480, 960, 1600, 2400`, formats AVIF
(quality 55) and JPEG (quality 82, always written: it is the fallback every browser reads), screen
sharpening, copyright metadata only, **GPS removed** (`keepLocation: true` keeps it).

**Alt text** comes from the caption, else the title, else the first six keywords, else the file name
made readable; the result says how many came from the file name only (`altFromFilename`), which are
the ones worth writing a caption for. The page title is `title`; `baseUrl` is written into the
JSON-LD (`https://site/galleries/food/`).

```sh
lightcraft-cli run --library ~/Pictures/Test.lcl library.filter keyword=plat \
    gallery.export dir=/tmp/food-gallery title="Plats" widths=[480,960,1600] baseUrl=https://example.com/food/
```

Parameters: `{dir, ids?, title?, widths?, formats?: [avif, webp, jpeg], quality?, avifQuality?,
baseUrl?, keepLocation?}`. WebP is lossless only in the encoder LightCraft ships (large files), so it
is not in the defaults. The command refuses a folder that would replace a photo's original.

In the app the window waits while the gallery is written (each photo is rendered at every width).
