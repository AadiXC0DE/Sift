use ammonia::Builder;
use base64::Engine;
use std::collections::HashSet;

pub struct SanitizeOut {
    pub html: String,
    pub remote_images: i64,
    pub trackers: i64,
    pub dark_safe: bool,
}

fn tracker_hosts() -> HashSet<String> {
    include_str!("../../assets/trackers.txt")
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
}

fn is_tracker(src: &str, hosts: &HashSet<String>) -> bool {
    let lower = src.to_lowercase();
    if lower.contains("/o.gif") || lower.contains("/open.php") {
        return true;
    }
    // host match
    if let Ok(url) = url::Url::parse(src) {
        if let Some(h) = url.host_str() {
            let h = h.to_lowercase();
            for pat in hosts {
                let p = pat.trim_matches('.');
                if pat.starts_with('.') || pat.starts_with("*.") {
                    if h.ends_with(p) || h == p.trim_start_matches("*.") {
                        return true;
                    }
                } else if h == *pat || h.ends_with(&format!(".{pat}")) {
                    return true;
                }
                if (h.starts_with("open.")
                    || h.starts_with("click.")
                    || h.starts_with("pixel.")
                    || h.starts_with("track."))
                    && h.contains('.')
                {
                    return true;
                }
            }
        }
    } else {
        for pat in ["open.", "click.", "pixel.", "track.", "/o.gif", "/open.php"] {
            if lower.contains(pat) {
                return true;
            }
        }
    }
    false
}

pub fn sanitize(message_id: &str, raw_html: &str) -> SanitizeOut {
    let hosts = tracker_hosts();
    // First pass: ammonia with custom config
    let mut builder = Builder::default();
    builder
        .add_tags([
            "table", "thead", "tbody", "tfoot", "tr", "td", "th", "col", "colgroup", "center",
            "font", "span", "div", "img",
        ])
        .rm_tags([
            "form", "input", "button", "select", "textarea", "iframe", "object", "embed", "script",
            "style", "link", "meta", "base", "svg", "math", "video", "audio",
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
            "color",
            "face",
            "size",
            "colspan",
            "rowspan",
            "dir",
            "lang",
        ])
        .add_tag_attributes("a", &["href"])
        .add_tag_attributes("img", &["src", "width", "height", "alt"])
        .add_tag_attributes("td", &["colspan", "rowspan"])
        .url_schemes(
            ["http", "https", "mailto", "tel", "cid", "sift-att", "data"]
                .into_iter()
                .collect(),
        )
        .link_rel(Some("noopener noreferrer"));
    // style filtering is done post-pass
    let mut html = builder.clean(raw_html).to_string();

    // Post-pass with a lightweight string walk for img/link/style rules.
    // We do targeted rewrites without a full DOM lib to keep build light.
    let mut remote_images = 0i64;
    let mut trackers = 0i64;

    // Rewrite cid: -> sift-att:// (strip <angle brackets> Gmail puts on Content-ID)
    html = rewrite_cid_srcs(html, message_id);

    // Process <img> tags: detect remote vs tracker
    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();
    while let Some(start) = rest.find("<img") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after.find('>').map(|i| i + 1).unwrap_or(after.len());
        let tag = &after[..end];
        rest = &after[end..];
        // extract src
        let src = extract_attr(tag, "src").unwrap_or_default();
        let w = extract_attr(tag, "width")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let h = extract_attr(tag, "height")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let is_remote = src.starts_with("http://") || src.starts_with("https://");
        let tiny = (w > 0 && w <= 2) && (h > 0 && h <= 2);
        if is_remote && (tiny || is_tracker(&src, &hosts)) {
            trackers += 1;
            continue; // drop
        }
        if is_remote {
            remote_images += 1;
            // move src -> data-sift-src with 1x1 placeholder
            let placeholder =
                "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
            let new_tag = tag
                .replacen(
                    &format!("src=\"{src}\""),
                    &format!(
                        "data-sift-src=\"{src}\" src=\"{placeholder}\" class=\"sift-blocked\""
                    ),
                    1,
                )
                .replacen(
                    &format!("src='{src}'"),
                    &format!("data-sift-src='{src}' src='{placeholder}' class='sift-blocked'"),
                    1,
                );
            // if src had no quotes (rare), inject
            if new_tag == tag {
                out.push_str(&tag.replacen(
                    "<img",
                    &format!(
                        "<img data-sift-src=\"{src}\" src=\"{placeholder}\" class=\"sift-blocked\""
                    ),
                    1,
                ));
            } else {
                out.push_str(&new_tag);
            }
            continue;
        }
        // data: images over 512KB -> drop
        if src.starts_with("data:") && src.len() > 512 * 1024 {
            trackers += 1;
            continue;
        }
        out.push_str(tag);
    }
    out.push_str(rest);
    html = out;

    // Force links target blank (ammonia link_rel set; ensure target)
    html = html.replace("<a ", "<a target=\"_blank\" ");

    // Strip dangerous style declarations
    html = strip_bad_styles(&html);

    // Quote wrapping: gmail_quote / blockquote cite -> details
    html = html.replace(
        "<div class=\"gmail_quote\">",
        "<details class=\"sift-quote\" open><summary>•••</summary>",
    );
    html = html.replace(
        "<blockquote type=\"cite\">",
        "<details class=\"sift-quote\"><summary>•••</summary><blockquote>",
    );

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

    // Strip comments
    while let Some(a) = html.find("<!--") {
        if let Some(b) = html[a..].find("-->") {
            html.replace_range(a..a + b + 3, "");
        } else {
            break;
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
        for k in [id.as_str(), part_id.as_str(), cid.as_deref().unwrap_or("")] {
            if k.is_empty() {
                continue;
            }
            html = html.replace(&format!("sift-att://{message_id}/{k}"), &data_url);
        }
    }
    html
}

fn strip_bad_styles(html: &str) -> String {
    // Remove style="..." declarations containing url(, expression(, @import, position:fixed/absolute
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
                    !(l.contains("url(")
                        || l.contains("expression(")
                        || l.contains("@import")
                        || l.contains("position:fixed")
                        || l.contains("position:absolute"))
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
        assert!(out
            .html
            .contains("data-sift-src=\"https://example.com/photo.jpg\""));
        assert!(!out.html.contains("open.tracker.com"));
        assert_eq!(out.tracker_count_plus(), 1);
        assert_eq!(out.remote_images, 1);
    }
    #[test]
    fn p3_t08_dark_safe() {
        let safe = sanitize("m", "<p style=\"color:#333\">hi</p>");
        assert!(safe.dark_safe);
        let unsafe_ = sanitize("m", "<div bgcolor=\"#123456\">hi</div>");
        assert!(!unsafe_.dark_safe);
    }
    impl SanitizeOut {
        fn tracker_count_plus(&self) -> i64 {
            self.trackers
        }
    }
}
