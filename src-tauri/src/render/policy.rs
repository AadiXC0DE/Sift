//! Remote-content policy for rendered mail (P9.1).
//!
//! Sanitized bodies are stored **policy-neutral**: they keep the message's own
//! `http(s)://` references so that a later read can render them without
//! re-fetching or re-rendering the body. Every read then applies the current
//! permission to that neutral HTML:
//!
//! * [`block_remote_content`] removes every external load vector, so a blocked
//!   read performs **zero** external requests even if a content policy is
//!   misconfigured downstream;
//! * [`allow_remote_content`] keeps the HTTPS image resources the message
//!   needs, while remote fonts, external stylesheets, media autoplay and
//!   embedded frames stay blocked in every mode.
//!
//! CSS background images follow exactly the same permission as `<img src>`,
//! and `@import` / `@font-face` are removed unconditionally.

/// Stand-in for a blocked image: a transparent 1x1 GIF that cannot mail home.
pub const BLOCKED_PIXEL: &str =
    "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/// Elements whose `src`/`poster`/`background` can start a network request.
const MEDIA_ELEMENTS: &[&str] = &["img", "source", "video", "audio", "track", "embed"];

/// Is this URL something the machine would have to fetch from the network?
pub fn is_external(url: &str) -> bool {
    let trimmed = url
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_ascii_lowercase();
    trimmed.starts_with("http:") || trimmed.starts_with("https:") || trimmed.starts_with("//")
}

/// Locate `name` as an attribute of `tag`, returning the byte span of the
/// whole attribute (name through closing quote, or just the name for a
/// valueless attribute such as `autoplay`) so it can be removed.
fn attr_span(tag: &str, name: &str) -> Option<(usize, usize)> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while from < lower.len() {
        let i = from + lower[from..].find(name)?;
        let before_ok =
            i == 0 || matches!(tag.as_bytes()[i - 1], b' ' | b'\t' | b'\n' | b'\r' | b'<');
        let after = i + name.len();
        if before_ok {
            match tag.as_bytes().get(after) {
                Some(b'=') => {
                    let value = tag[after + 1..].trim_start();
                    let quote = value.chars().next()?;
                    if quote == '"' || quote == '\'' {
                        let rest = &value[1..];
                        let end = rest.find(quote)?;
                        return Some((i, after + 1 + 1 + end + 1));
                    }
                    let end = value
                        .find(|c: char| c.is_whitespace() || c == '>')
                        .unwrap_or(value.len());
                    return Some((i, after + 1 + end));
                }
                // A valueless attribute (`autoplay`, `controls`) ends at the
                // next whitespace or the end of the tag.
                Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'>') | Some(b'/') => {
                    return Some((i, after))
                }
                _ => {}
            }
        }
        from = i + name.len();
    }
    None
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let (start, end) = attr_span(tag, name)?;
    let span = &tag[start..end];
    let (_, value) = span.split_once('=')?;
    let value = value.trim();
    let value = value.trim_matches(|c| c == '"' || c == '\'');
    Some(value.to_string())
}

/// Remove an attribute, plus the whitespace that preceded it.
fn remove_attr(tag: &str, name: &str) -> String {
    match attr_span(tag, name) {
        Some((start, end)) => {
            let mut before = start;
            while before > 0 && tag.as_bytes()[before - 1].is_ascii_whitespace() {
                before -= 1;
            }
            format!("{}{}", &tag[..before], &tag[end..])
        }
        None => tag.to_string(),
    }
}

fn set_attr(tag: &str, name: &str, value: &str) -> String {
    match attr_span(tag, name) {
        Some((start, end)) => format!("{}{}=\"{}\"{}", &tag[..start], name, value, &tag[end..]),
        None => {
            let split = tag.trim_end_matches('>').trim_end().len();
            format!("{} {}=\"{}\"{}", &tag[..split], name, value, &tag[split..])
        }
    }
}

/// Rebuild one element tag under the image permission.
fn rewrite_element(tag: &str, allow_images: bool) -> String {
    let name = tag
        .trim_start_matches('<')
        .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut out = tag.to_string();

    // Media playback hints are never reintroduced, in any policy.
    out = remove_attr(&out, "autoplay");
    out = remove_attr(&out, "preload");

    let mut external_attrs: Vec<&str> = Vec::new();
    if MEDIA_ELEMENTS.contains(&name.as_str()) {
        external_attrs.extend(["src", "poster", "data"]);
    }
    if matches!(name.as_str(), "table" | "td" | "th" | "tr" | "body" | "div") {
        external_attrs.push("background");
    }

    for attribute in external_attrs {
        let Some(value) = attr_value(&out, attribute) else {
            continue;
        };
        if !is_external(&value) || allow_images {
            continue;
        }
        if attribute == "src" && name == "img" {
            // A transparent local pixel keeps the message's layout while
            // guaranteeing the read performs no external request.
            out = set_attr(&out, "src", BLOCKED_PIXEL);
        } else {
            out = remove_attr(&out, attribute);
        }
    }

    if let Some(srcset) = attr_value(&out, "srcset") {
        if is_external(&srcset) && !allow_images {
            out = remove_attr(&out, "srcset");
        }
    }
    out
}

/// Rewrite every element tag in the document.
fn rewrite_elements(html: &str, allow_images: bool) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find('>') else {
            out.push_str(after);
            return out;
        };
        let tag = &after[..=end];
        if tag.starts_with("</") || tag.starts_with("<!") || tag.starts_with("<?") {
            out.push_str(tag);
        } else {
            out.push_str(&rewrite_element(tag, allow_images));
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Strip the at-rules that can load something from the network regardless of
/// permission: external stylesheets and remote fonts.
fn strip_at_rules(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let lower = css.to_ascii_lowercase();
    let bytes = css.as_bytes();
    let mut i = 0usize;
    while i < css.len() {
        if lower[i..].starts_with("@import") || lower[i..].starts_with("@font-face") {
            let is_font_face = lower[i..].starts_with("@font-face");
            if is_font_face {
                // Remove the whole balanced block.
                match lower[i..].find('{') {
                    Some(open) => {
                        let mut depth = 0i32;
                        let mut j = i + open;
                        while j < bytes.len() {
                            match bytes[j] {
                                b'{' => depth += 1,
                                b'}' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        j += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            j += 1;
                        }
                        i = j;
                    }
                    None => {
                        i = css.len();
                    }
                }
            } else {
                // `@import url(...) screen;` — remove through the semicolon.
                match css[i..].find(';') {
                    Some(end) => i += end + 1,
                    None => i = css.len(),
                }
            }
            continue;
        }
        let ch = css[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn find_css_url(css: &str, from: usize) -> Option<(usize, usize, String)> {
    let lower = css.to_ascii_lowercase();
    let rel = lower[from..].find("url(")?;
    let open = from + rel + 4;
    let close = css[open..].find(')')? + open;
    let raw = css[open..close].trim();
    let value = raw.trim_matches(|c| c == '"' || c == '\'').to_string();
    Some((from + rel, close + 1, value))
}

/// Apply the image permission to every `url(...)` in one CSS fragment.
fn rewrite_css_urls(css: &str, allow_images: bool) -> String {
    let mut out = String::with_capacity(css.len());
    let mut from = 0usize;
    loop {
        match find_css_url(css, from) {
            Some((start, end, value)) => {
                out.push_str(&css[from..start]);
                if is_external(&value) && !allow_images {
                    out.push_str(&format!("url(\"{BLOCKED_PIXEL}\")"));
                } else {
                    out.push_str(&css[start..end]);
                }
                from = end;
            }
            None => {
                out.push_str(&css[from..]);
                break;
            }
        }
    }
    out
}

/// Rewrite every `style="…"` attribute and every `<style>` block.
fn rewrite_css(html: &str, allow_images: bool) -> String {
    let html = rewrite_style_attributes(html, allow_images);
    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find("<style") else {
            out.push_str(rest);
            break;
        };
        let Some(open_end) = rest[start..].find('>') else {
            out.push_str(rest);
            break;
        };
        let content_start = start + open_end + 1;
        let lower_rest = rest[content_start..].to_ascii_lowercase();
        let Some(close) = lower_rest.find("</style") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..content_start]);
        let css = &rest[content_start..content_start + close];
        out.push_str(&rewrite_css_urls(&strip_at_rules(css), allow_images));
        rest = &rest[content_start + close..];
    }
    out
}

fn rewrite_style_attributes(html: &str, allow_images: bool) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("style=\"") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 7..];
        let Some(end) = after.find('"') else {
            out.push_str(rest);
            return out;
        };
        out.push_str("style=\"");
        out.push_str(&rewrite_css_urls(
            &strip_at_rules(&after[..end]),
            allow_images,
        ));
        out.push('"');
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Remove the load vectors that are blocked in every policy. Applied when a
/// body is stored, so a cached body never carries an external stylesheet or a
/// remote font.
pub fn strip_blocked_vectors(html: &str) -> String {
    rewrite_css(html, true)
}

/// Render a stored body with external content blocked. The result contains no
/// addressable external URL: blocked images become a local data URI, blocked
/// CSS backgrounds likewise, and media sources are removed.
pub fn block_remote_content(html: &str) -> String {
    let stripped = rewrite_css(html, false);
    rewrite_elements(&stripped, false)
}

/// Render a stored body with the HTTPS resources a message needs permitted.
pub fn allow_remote_content(html: &str) -> String {
    let stripped = rewrite_css(html, true);
    rewrite_elements(&stripped, true)
}

/// Apply the current permission to a stored body.
pub fn apply(html: &str, allow_images: bool) -> String {
    if allow_images {
        allow_remote_content(html)
    } else {
        block_remote_content(html)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_removes_every_external_load() {
        let html = concat!(
            "<img src=\"https://cdn.example/a.png\" srcset=\"https://cdn.example/a2.png 2x\">",
            "<video src=\"https://cdn.example/v.mp4\" poster=\"https://cdn.example/p.jpg\"></video>",
            "<source src=\"https://cdn.example/s.mp3\" srcset=\"https://cdn.example/x.mp3 2x\">",
            "<div style=\"background-image:url(https://cdn.example/bg.png)\">x</div>",
            "<table background=\"https://cdn.example/t.png\"><tr><td>y</td></tr></table>",
        );
        let out = block_remote_content(html);
        assert!(!out.contains("https://cdn.example"), "{out}");
        assert!(out.contains(BLOCKED_PIXEL), "{out}");
        assert!(out.contains("<video"), "{out}");
        assert!(!out.contains("srcset"), "{out}");
    }

    #[test]
    fn allowed_keeps_images_but_never_applauds_fonts_or_imports() {
        let html = concat!(
            "<img src=\"https://cdn.example/a.png\" srcset=\"https://cdn.example/a2.png 2x\">",
            "<style>@import url(https://cdn.example/x.css);@font-face{font-family:X;src:url(https://cdn.example/f.woff2)}.a{background:url(https://cdn.example/b.png)}</style>",
        );
        let out = allow_remote_content(html);
        assert!(out.contains("src=\"https://cdn.example/a.png\""), "{out}");
        assert!(out.contains("url(https://cdn.example/b.png)"), "{out}");
        assert!(!out.contains("@import"), "{out}");
        assert!(!out.contains("@font-face"), "{out}");
        assert!(!out.contains("f.woff2"), "{out}");
    }

    #[test]
    fn media_playback_hints_never_survive() {
        let out = allow_remote_content("<video src=\"https://x/v.mp4\" autoplay preload=\"auto\">");
        assert!(!out.contains("autoplay"), "{out}");
        assert!(!out.contains("preload"), "{out}");
        assert!(out.contains("https://x/v.mp4"), "{out}");
    }

    #[test]
    fn local_references_are_untouched() {
        let html = "<img src=\"sift-att://a1/m1/ii_1\"><img src=\"data:image/png;base64,AA\">";
        assert_eq!(block_remote_content(html), html);
    }
}
