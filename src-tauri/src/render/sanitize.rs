use ammonia::Builder;
use base64::Engine;

pub struct SanitizeOut {
    pub html: String,
    pub remote_images: i64,
    pub trackers: i64,
    pub dark_safe: bool,
}

pub fn sanitize(message_id: &str, raw_html: &str) -> SanitizeOut {
    // First pass: ammonia with custom config
    let mut builder = Builder::default();
    builder
        .rm_clean_content_tags(["style"])
        .add_tags([
            "table",
            "thead",
            "tbody",
            "tfoot",
            "tr",
            "td",
            "th",
            "col",
            "colgroup",
            "center",
            "font",
            "span",
            "div",
            "img",
            "style",
            "p",
            "br",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "strong",
            "em",
            "b",
            "i",
            "u",
            "s",
            "blockquote",
            "pre",
            "code",
            "hr",
            "ul",
            "ol",
            "li",
            "dl",
            "dt",
            "dd",
            "main",
            "section",
            "article",
            "header",
            "footer",
            "nav",
            "figure",
            "figcaption",
            "picture",
            "source",
            "map",
            "area",
            "video",
            "audio",
        ])
        .rm_tags([
            "form", "input", "button", "select", "textarea", "iframe", "object", "embed", "script",
            "link", "meta", "base", "svg", "math",
        ])
        .add_generic_attributes([
            "class",
            "id",
            "title",
            "alt",
            "width",
            "height",
            "align",
            "valign",
            "border",
            "cellpadding",
            "cellspacing",
            "bgcolor",
            "background",
            "color",
            "face",
            "size",
            "colspan",
            "rowspan",
            "dir",
            "lang",
            "style",
            "role",
            "aria-label",
            "aria-hidden",
            "hidden",
        ])
        .add_tag_attributes("a", &["href", "target", "name"])
        .add_tag_attributes(
            "img",
            &[
                "src", "srcset", "sizes", "width", "height", "alt", "border", "hspace", "vspace",
                "usemap", "ismap", "loading", "decoding",
            ],
        )
        .add_tag_attributes("source", &["src", "srcset", "sizes", "type", "media"])
        .add_tag_attributes("video", &["src", "poster", "width", "height", "controls"])
        .add_tag_attributes("audio", &["src", "controls"])
        .add_tag_attributes("style", &["type", "media"])
        .add_tag_attributes(
            "table",
            &[
                "summary",
                "rules",
                "frame",
                "width",
                "height",
                "bgcolor",
                "background",
            ],
        )
        .add_tag_attributes(
            "td",
            &[
                "colspan", "rowspan", "headers", "scope", "abbr", "axis", "nowrap",
            ],
        )
        .url_schemes(
            ["http", "https", "mailto", "tel", "cid", "sift-att", "data"]
                .into_iter()
                .collect(),
        )
        .link_rel(Some("noopener noreferrer"));
    // style filtering is done post-pass
    let mut html = builder.clean(raw_html).to_string();

    let mut remote_images = 0i64;
    let trackers = 0i64;

    // Rewrite cid: -> sift-att:// (strip <angle brackets> Gmail puts on Content-ID)
    html = rewrite_cid_srcs(html, message_id);

    // Keep every image. Rendering wins over tracker stripping.
    let mut rest = html.as_str();
    while let Some(start) = rest.find("<img") {
        let after = &rest[start..];
        let end = after.find('>').map(|i| i + 1).unwrap_or(after.len());
        let tag = &after[..end];
        rest = &after[end..];
        let src = extract_attr(tag, "src").unwrap_or_default();
        if src.starts_with("http://") || src.starts_with("https://") {
            remote_images += 1;
        }
    }

    // Protocol-relative assets resolve against Tauri's custom app scheme in a
    // srcdoc iframe. Normalize them to HTTPS so CDN-hosted email assets load.
    html = normalize_protocol_relative_urls(html);

    // Keep authored layout CSS. Strip only legacy active-CSS script vectors;
    // iframe CSP and sandboxing provide the primary execution boundary.
    html = strip_bad_styles(&html);

    // dark_safe: no bgcolor/background other than white/transparent
    let lower = html.to_lowercase();
    let mut dark_safe = true;
    for marker in ["bgcolor=", "background:", "background-color:"] {
        if lower.contains(marker) {
            // allow white/transparent only
            let idx = lower.find(marker).unwrap();
            let snippet = &lower[idx..(idx + 60).min(lower.len())];
            if !(snippet.contains("#fff")
                || snippet.contains("#ffffff")
                || snippet.contains("white")
                || snippet.contains("transparent"))
            {
                dark_safe = false;
                break;
            }
        }
    }

    SanitizeOut {
        html,
        remote_images,
        trackers,
        dark_safe,
    }
}

fn extract_attr(tag: &str, name: &str) -> Option<String> {
    for q in ['"', '\''] {
        let pat = format!("{name}={q}");
        if let Some(i) = tag.find(&pat) {
            let rest = &tag[i + pat.len()..];
            if let Some(e) = rest.find(q) {
                return Some(rest[..e].to_string());
            }
        }
    }
    None
}

fn rewrite_cid_srcs(html: String, message_id: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();
    while let Some(i) = rest.find("cid:") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 4..];
        let mut end = after
            .find(['"', '\'', ' ', '>', '&'])
            .unwrap_or(after.len());
        let raw = &after[..end];
        let cid = raw.trim_matches(|c: char| c == '<' || c == '>');
        if raw.starts_with('<') && after[end..].starts_with('>') {
            end += 1;
        }
        out.push_str(&format!("sift-att://{message_id}/{cid}"));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn normalize_protocol_relative_urls(html: String) -> String {
    html.replace("=\"//", "=\"https://")
        .replace("='//", "='https://")
        .replace("url(//", "url(https://")
        .replace("url(&quot;//", "url(&quot;https://")
        .replace("url(&#x27;//", "url(&#x27;https://")
}

/// Turn blocked remote images back into real `src` (Load / always-allow).
pub fn restore_remote_images(html: &str) -> String {
    const PLACEHOLDER: &str =
        "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
    html.replace(&format!("src=\"{PLACEHOLDER}\""), "")
        .replace(&format!("src='{PLACEHOLDER}'"), "")
        .replace(" class=\"sift-blocked\"", "")
        .replace(" class='sift-blocked'", "")
        .replace("data-sift-src=\"", "src=\"")
        .replace("data-sift-src='", "src='")
}

/// srcdoc iframes cannot load custom `sift-att://` URLs; embed bytes as data URIs.
pub type InlinePart = (String, String, Option<String>, String, Vec<u8>);

pub fn embed_local_images(html: &str, message_id: &str, parts: &[InlinePart]) -> String {
    let mut html = html.to_string();
    let mut replacements = Vec::new();
    for (id, part_id, content_id, mime, data) in parts {
        if data.is_empty() || !mime.starts_with("image/") {
            continue;
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(data);
        let data_url = format!("data:{mime};base64,{b64}");
        let cid = content_id.as_ref().map(|c| {
            c.trim()
                .trim_matches(|ch: char| ch == '<' || ch == '>')
                .to_string()
        });
        for key in [id.as_str(), part_id.as_str(), cid.as_deref().unwrap_or("")] {
            if !key.is_empty() {
                replacements.push((key.to_string(), data_url.clone()));
            }
        }
    }
    // Longest first avoids corrupting a reference whose key has a shorter key
    // as its prefix (for example, `image` and `image-large`).
    replacements.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
    for (key, data_url) in replacements {
        html = replace_exact_attachment_ref(&html, message_id, &key, &data_url);
    }
    html
}

fn replace_exact_attachment_ref(html: &str, message_id: &str, key: &str, value: &str) -> String {
    let needle = format!("sift-att://{message_id}/{key}");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(&needle) {
        out.push_str(&rest[..start]);
        let after = &rest[start + needle.len()..];
        let boundary = after
            .chars()
            .next()
            .map(|c| matches!(c, '\"' | '\'' | ' ' | '>' | ')' | '&'))
            .unwrap_or(true);
        if boundary {
            out.push_str(value);
        } else {
            out.push_str(&needle);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

pub fn inline_image_refs(html: &str, message_id: &str) -> Vec<String> {
    let prefix = format!("sift-att://{message_id}/");
    let mut refs = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find(&prefix) {
        let value = &rest[start + prefix.len()..];
        let end = value
            .find(['\"', '\'', ' ', '>', ')', '&'])
            .unwrap_or(value.len());
        let key = value[..end].trim_matches(|c: char| c == '<' || c == '>');
        if !key.is_empty() && !refs.iter().any(|existing| existing == key) {
            refs.push(key.to_string());
        }
        rest = &value[end..];
    }
    refs
}

fn strip_bad_styles(html: &str) -> String {
    // Keep layout CSS (including url() backgrounds). Drop only active XSS vectors.
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("style=\"") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 7..];
        if let Some(e) = after.find('"') {
            let style = &after[..e];
            let kept: Vec<&str> = style
                .split(';')
                .filter(|d| {
                    let l = d.to_lowercase();
                    !(l.contains("expression(")
                        || l.contains("javascript:")
                        || l.contains("@import")
                        || l.contains("behavior:")
                        || l.contains("-moz-binding"))
                })
                .collect();
            out.push_str(&format!("style=\"{}\"", kept.join(";")));
            rest = &after[e + 1..];
        } else {
            out.push_str(rest);
            break;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn p3_t07_sanitizer_basics() {
        let out = sanitize("m1", "<p>hi</p><script>window.__sift_xss=1</script><a href=\"javascript:alert(1)\">x</a><form><input></form><img src=\"cid:ii_1\" width=\"10\" height=\"10\">");
        assert!(!out.html.contains("<script"));
        assert!(!out.html.contains("javascript:"));
        assert!(!out.html.contains("<form"));
        assert!(!out.html.contains("onload="));
        assert!(out.html.contains("sift-att://m1/ii_1"));
    }
    #[test]
    fn rewrite_cid_strips_brackets() {
        let out = rewrite_cid_srcs("<img src=\"cid:<ii_1>\">".into(), "m1");
        assert_eq!(out, "<img src=\"sift-att://m1/ii_1\">");
    }
    #[test]
    fn restore_remote_keeps_one_src() {
        let blocked = sanitize(
            "m1",
            "<img src=\"https://example.com/photo.jpg\" width=\"100\" height=\"50\">",
        );
        let restored = restore_remote_images(&blocked.html);
        assert!(restored.contains("src=\"https://example.com/photo.jpg\""));
        assert!(!restored.contains("data-sift-src"));
        assert!(!restored.contains("sift-blocked"));
        assert_eq!(restored.matches("src=\"").count(), 1);
    }
    #[test]
    fn embed_cid_as_data_uri() {
        let html = "<img src=\"sift-att://m1/ii_1\">";
        let out = embed_local_images(
            html,
            "m1",
            &[(
                "att1".into(),
                "p1".into(),
                Some("ii_1".into()),
                "image/png".into(),
                vec![1, 2, 3],
            )],
        );
        assert!(out.starts_with("<img src=\"data:image/png;base64,"));
        assert!(!out.contains("sift-att://"));
    }
    #[test]
    fn p3_t07_remote_and_tracker() {
        let out = sanitize("m1", "<img src=\"https://example.com/photo.jpg\" width=\"100\" height=\"50\"><img src=\"https://open.tracker.com/o.gif\" width=\"1\" height=\"1\">");
        assert!(out.html.contains("https://example.com/photo.jpg"));
        assert!(out.html.contains("open.tracker.com"));
        assert_eq!(out.trackers, 0);
        assert_eq!(out.remote_images, 2);
    }
    #[test]
    fn p3_t08_dark_safe() {
        let safe = sanitize("m", "<p style=\"color:#333\">hi</p>");
        assert!(safe.dark_safe);
        let unsafe_ = sanitize("m", "<div bgcolor=\"#123456\">hi</div>");
        assert!(!unsafe_.dark_safe);
    }
    #[test]
    fn keeps_email_css() {
        let out = sanitize(
            "m1",
            "<style>.x{color:red}</style><p style=\"color:#333;background-image:url(https://cdn.example/a.png)\">hi</p>",
        );
        assert!(out.html.contains("style=\"color:#333"));
        assert!(out.html.contains("background-image:url("));
        assert!(out.html.contains("<style"));
        assert!(out.html.contains(".x{color:red}") || out.html.contains(".x { color: red }"));
    }

    #[test]
    fn preserves_email_layout_attributes_and_media() {
        let out = sanitize(
            "m1",
            "<style media=\"screen\">@media(max-width:600px){.hero{width:100%}}</style><table width=\"600\" cellspacing=\"12\" cellpadding=\"8\" bgcolor=\"#ffffff\" role=\"presentation\"><tr><td nowrap background=\"https://cdn.example/bg.png\"><picture><source media=\"(min-width:600px)\" srcset=\"https://cdn.example/hero@2x.png 2x\"><img class=\"hero\" src=\"https://cdn.example/hero.png\" width=\"600\" height=\"240\" hspace=\"0\"></picture></td></tr></table>",
        );
        for expected in [
            "width=\"600\"",
            "cellspacing=\"12\"",
            "cellpadding=\"8\"",
            "bgcolor=\"#ffffff\"",
            "nowrap",
            "<picture>",
            "<source",
            "srcset=\"https://cdn.example/hero@2x.png 2x\"",
            "height=\"240\"",
        ] {
            assert!(
                out.html.contains(expected),
                "missing {expected}: {}",
                out.html
            );
        }
        assert!(!out.html.contains("sift-quote"));
    }

    #[test]
    fn normalizes_protocol_relative_assets() {
        let out = sanitize(
            "m1",
            "<img src=\"//cdn.example/a.png\"><div style=\"background:url(//cdn.example/b.png)\"></div>",
        );
        assert!(out.html.contains("src=\"https://cdn.example/a.png\""));
        assert!(out.html.contains("url(https://cdn.example/b.png)"));
    }

    #[test]
    fn inline_refs_and_prefix_keys_are_safe() {
        let html = "<img src=\"sift-att://m1/image-large\"><img src=\"sift-att://m1/image\">";
        assert_eq!(
            inline_image_refs(html, "m1"),
            vec!["image-large".to_string(), "image".to_string()]
        );
        let out = embed_local_images(
            html,
            "m1",
            &[
                (
                    "image".into(),
                    "image".into(),
                    None,
                    "image/png".into(),
                    vec![1],
                ),
                (
                    "image-large".into(),
                    "image-large".into(),
                    None,
                    "image/jpeg".into(),
                    vec![2],
                ),
            ],
        );
        assert!(!out.contains("sift-att://"));
        assert_eq!(out.matches("data:image/").count(), 2);

        let short_only = embed_local_images(
            html,
            "m1",
            &[(
                "image".into(),
                "image".into(),
                None,
                "image/png".into(),
                vec![1],
            )],
        );
        assert!(short_only.contains("sift-att://m1/image-large"));
        assert!(!short_only.contains("data:image/png;base64,AQ==-large"));
    }
}
