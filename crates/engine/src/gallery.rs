//! Static web gallery: the markup, JSON and snippet of an export of photos at several widths and
//! formats (`gallery.export`). Pure text generation, so it is tested without photos.
//!
//! The gallery is a folder: `index.html` (a self-contained page, one small inline script for the
//! lightbox, no external requests), `snippet.html` (the same grid without script, to paste in an
//! Odoo page or any CMS), `gallery.json` (machine-readable list) and `img/` (the files).

use serde_json::{Value, json};

/// One encoded file of a photo.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    /// Path relative to the gallery folder (`img/plat-960.avif`).
    pub file: String,
    pub mime: &'static str,
    pub width: usize,
    pub height: usize,
    pub bytes: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub name: String,
    pub alt: String,
    /// Where the alt text came from: `caption`, `title`, `keywords` or `filename`.
    pub alt_source: &'static str,
    pub caption: String,
    pub width: usize,
    pub height: usize,
    pub variants: Vec<Variant>,
}

pub fn escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// An alt text for a photo: its caption, else its title, else its first keywords, else the
/// file name made readable (`IMG_0004.CR2` → `IMG 0004`). Returns the text and where it came from.
pub fn alt_text(caption: &str, title: &str, keywords: &[String], file_name: &str) -> (String, &'static str) {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (c, t) = (one_line(caption), one_line(title));
    if !c.is_empty() {
        let c: String = c.chars().take(200).collect();
        return (c, "caption");
    }
    if !t.is_empty() {
        return (t, "title");
    }
    let k: Vec<&str> = keywords.iter().map(|k| k.trim()).filter(|k| !k.is_empty()).take(6).collect();
    if !k.is_empty() {
        return (k.join(", "), "keywords");
    }
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    (stem.replace(['_', '-'], " "), "filename")
}

fn mime_order(m: &str) -> usize {
    match m {
        "image/avif" => 0,
        "image/webp" => 1,
        _ => 2,
    }
}

fn srcset(vs: &[&Variant]) -> String {
    let mut v: Vec<&&Variant> = vs.iter().collect();
    v.sort_by_key(|x| x.width);
    v.iter().map(|x| format!("{} {}w", x.file, x.width)).collect::<Vec<_>>().join(", ")
}

/// `<picture>` for one entry: one `<source>` per modern format, the last format (JPEG) in the `<img>`.
pub fn picture(e: &Entry, sizes: &str, lazy: bool) -> String {
    let mut mimes: Vec<&'static str> = e.variants.iter().map(|v| v.mime).collect();
    mimes.sort_by_key(|m| mime_order(m));
    mimes.dedup();
    let Some(&fallback) = mimes.last() else { return String::new() };
    let of = |m: &str| e.variants.iter().filter(|v| v.mime == m).collect::<Vec<_>>();
    let mut out = String::from("<picture>");
    for m in &mimes[..mimes.len() - 1] {
        out.push_str(&format!("<source type=\"{m}\" srcset=\"{}\" sizes=\"{}\">", escape(&srcset(&of(m))), escape(sizes)));
    }
    let fb = of(fallback);
    let mid = {
        let mut s = fb.clone();
        s.sort_by_key(|v| v.width);
        s.get(s.len() / 2).map(|v| (*v).clone())
    };
    if let Some(d) = mid {
        out.push_str(&format!(
            "<img src=\"{}\" srcset=\"{}\" sizes=\"{}\" width=\"{}\" height=\"{}\" alt=\"{}\"{}>",
            escape(&d.file),
            escape(&srcset(&fb)),
            escape(sizes),
            d.width,
            d.height,
            escape(&e.alt),
            if lazy { " loading=\"lazy\" decoding=\"async\"" } else { "" }
        ));
    }
    out.push_str("</picture>");
    out
}

const SIZES: &str = "(min-width: 1200px) 31vw, (min-width: 700px) 48vw, 96vw";

const CSS: &str = ".lc-gallery{columns:3 280px;column-gap:10px;margin:0;padding:0}\
.lc-gallery figure{break-inside:avoid;margin:0 0 10px;position:relative}\
.lc-gallery img{display:block;width:100%;height:auto;background:#ececec}\
.lc-gallery figcaption{font:13px/1.4 system-ui,sans-serif;color:#555;padding:4px 2px}\
.lc-gallery a{display:block;cursor:zoom-in}";

/// The grid alone (no script): `<section class="lc-gallery">` with its style.
pub fn snippet(entries: &[Entry]) -> String {
    let mut s = format!("<style>{CSS}</style>\n<section class=\"lc-gallery\">\n");
    for e in entries {
        s.push_str("<figure>");
        s.push_str(&picture(e, SIZES, true));
        if !e.caption.trim().is_empty() {
            s.push_str(&format!("<figcaption>{}</figcaption>", escape(e.caption.trim())));
        }
        s.push_str("</figure>\n");
    }
    s.push_str("</section>\n");
    s
}

/// The full page: header, the grid and a keyboard-friendly lightbox (one `<dialog>`, ~25 lines of script).
pub fn page(title: &str, entries: &[Entry], base_url: &str) -> String {
    let ld: Vec<Value> = entries
        .iter()
        .filter_map(|e| {
            let best = e.variants.iter().filter(|v| v.mime == "image/jpeg").max_by_key(|v| v.width).or_else(|| e.variants.iter().max_by_key(|v| v.width))?;
            Some(json!({"@type": "ImageObject", "contentUrl": format!("{}{}", base_url, best.file), "name": e.alt, "width": best.width, "height": best.height}))
        })
        .collect();
    let ld = json!({"@context": "https://schema.org", "@type": "ImageGallery", "name": title, "image": ld}).to_string().replace("</", "<\\/");
    let mut body = String::new();
    for (i, e) in entries.iter().enumerate() {
        body.push_str(&format!("<figure data-i=\"{i}\">"));
        body.push_str(&picture(e, SIZES, i > 2));
        if !e.caption.trim().is_empty() {
            body.push_str(&format!("<figcaption>{}</figcaption>", escape(e.caption.trim())));
        }
        body.push_str("</figure>\n");
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{t}</title><style>body{{margin:0;padding:16px;background:#fff;color:#222;font-family:system-ui,sans-serif}}h1{{font-size:20px;font-weight:600;margin:0 0 14px}}\
{CSS}dialog{{border:0;padding:0;background:#000;max-width:100vw;max-height:100vh;width:100vw;height:100vh}}dialog img{{width:100%;height:100%;object-fit:contain;background:#000}}\
dialog::backdrop{{background:#000}}</style>\
<script type=\"application/ld+json\">{ld}</script></head><body><h1>{t}</h1>\n<section class=\"lc-gallery\">\n{body}</section>\n\
<dialog id=\"lb\"><picture id=\"lbp\"></picture></dialog>\n\
<script>(function(){{var d=document.getElementById('lb'),p=document.getElementById('lbp'),f=[].slice.call(document.querySelectorAll('.lc-gallery figure')),k=0;\
function show(i){{k=(i+f.length)%f.length;p.innerHTML=f[k].querySelector('picture').innerHTML.replace(/ loading=\"lazy\"/,'').replace(/sizes=\"[^\"]*\"/g,'sizes=\"100vw\"');if(!d.open)d.showModal()}}\
f.forEach(function(e,i){{e.addEventListener('click',function(){{show(i)}})}});\
d.addEventListener('click',function(){{d.close()}});\
document.addEventListener('keydown',function(e){{if(!d.open)return;if(e.key==='ArrowRight')show(k+1);if(e.key==='ArrowLeft')show(k-1)}})}})();</script>\
</body></html>\n",
        t = escape(title)
    )
}

pub fn manifest(title: &str, entries: &[Entry]) -> Value {
    json!({
        "title": title,
        "photos": entries.iter().map(|e| json!({
            "id": e.id, "name": e.name, "alt": e.alt, "altSource": e.alt_source, "caption": e.caption,
            "width": e.width, "height": e.height,
            "files": e.variants.iter().map(|v| json!({"file": v.file, "type": v.mime, "width": v.width, "height": v.height, "bytes": v.bytes})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        let v = |f: &str, m, w| Variant { file: f.into(), mime: m, width: w, height: w * 2 / 3, bytes: 1000 };
        Entry {
            id: 7,
            name: "plat".into(),
            alt: "Un plat <fini> \"zenith\"".into(),
            alt_source: "caption",
            caption: "Plat fini".into(),
            width: 1600,
            height: 1067,
            variants: vec![
                v("img/plat-480.avif", "image/avif", 480),
                v("img/plat-960.avif", "image/avif", 960),
                v("img/plat-480.jpg", "image/jpeg", 480),
                v("img/plat-960.jpg", "image/jpeg", 960),
                v("img/plat-1600.jpg", "image/jpeg", 1600),
            ],
        }
    }

    #[test]
    fn picture_lists_modern_formats_first_and_escapes_alt() {
        let p = picture(&entry(), "100vw", true);
        assert!(p.starts_with("<picture><source type=\"image/avif\""), "{p}");
        assert!(p.contains("img/plat-480.avif 480w, img/plat-960.avif 960w"), "{p}");
        assert!(p.contains("<img src=\"img/plat-960.jpg\""), "{p}");
        assert!(p.contains("width=\"960\" height=\"640\""), "{p}");
        assert!(p.contains("alt=\"Un plat &lt;fini&gt; &quot;zenith&quot;\""), "{p}");
        assert!(p.contains("loading=\"lazy\""));
        assert!(!p.contains("<fini>"));
    }

    #[test]
    fn page_snippet_and_manifest_agree_and_the_snippet_has_no_script() {
        let es = vec![entry()];
        let page = page("Food <shoot>", &es, "https://x.test/g/");
        assert!(page.contains("<title>Food &lt;shoot&gt;</title>"));
        assert!(page.contains("\"contentUrl\":\"https://x.test/g/img/plat-1600.jpg\""));
        assert!(page.contains("<dialog"));
        let snip = snippet(&es);
        assert!(!snip.contains("<script"));
        assert!(snip.contains("<figcaption>Plat fini</figcaption>"));
        let m = manifest("t", &es);
        assert_eq!(m["photos"][0]["files"].as_array().map(Vec::len), Some(5));
        assert_eq!(m["photos"][0]["altSource"], "caption");
    }

    #[test]
    fn alt_text_prefers_caption_then_title_then_keywords_then_the_name() {
        let kw = vec!["plat".to_string(), " ".to_string(), "zénith".to_string()];
        assert_eq!(alt_text("  A  dish\non a table ", "T", &kw, "IMG_1.CR2"), ("A dish on a table".into(), "caption"));
        assert_eq!(alt_text("", "Title", &kw, "IMG_1.CR2"), ("Title".into(), "title"));
        assert_eq!(alt_text("", "", &kw, "IMG_1.CR2"), ("plat, zénith".into(), "keywords"));
        assert_eq!(alt_text("", "", &[], "IMG_0004.CR2"), ("IMG 0004".into(), "filename"));
    }
}
