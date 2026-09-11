//! text/plain -> HTML with auto-links, whitespace, quote collapse.
//!
//! Plain-text mail is rendered with `white-space: pre-wrap`, so authored
//! indentation, code blocks, and ASCII tables survive. URLs are auto-linked
//! without collapsing surrounding whitespace. The first quoted-reply marker
//! (`>` lines, `On … wrote:`, `-----Original Message-----`) starts a collapsed
//! `<details>` block, matching how Apple Mail and Gmail treat replies.

pub fn to_html(text: &str) -> (String, Option<usize>) {
    let mut head = String::new();
    let mut quoted = String::new();
    let mut quoted_from: Option<usize> = None;

    for (i, line) in text.lines().enumerate() {
        let marker = is_quote_marker(line);
        if marker && quoted_from.is_none() {
            quoted_from = Some(i);
        }
        let buf = if quoted_from.is_some() {
            &mut quoted
        } else {
            &mut head
        };
        buf.push_str(&linkify_escaped(line));
        buf.push('\n');
    }

    let mut html = String::from("<div class=\"sift-plain\">");
    html.push_str(&head);
    if quoted_from.is_some() {
        html.push_str(
            "<details class=\"sift-quote\"><summary>Show quoted text</summary><div class=\"sift-quote-body\">",
        );
        html.push_str(&quoted);
        html.push_str("</div></details>");
    }
    html.push_str("</div>");
    (html, quoted_from)
}

/// True when a line begins the quoted history of a reply.
fn is_quote_marker(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('>')
        || t == "-----Original Message-----"
        || (t.starts_with("On ") && t.ends_with("wrote:"))
        || line.starts_with("From: ")
}

/// Escape a line and wrap bare http(s) URLs in anchors, preserving the exact
/// whitespace and punctuation around them.
fn linkify_escaped(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 16);
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if rest.starts_with("http://") || rest.starts_with("https://") {
            // A URL runs to the next whitespace or angle bracket.
            let end_rel = rest
                .find(|c: char| c.is_whitespace() || c == '<' || c == '>')
                .unwrap_or(rest.len());
            // Trim common trailing punctuation that is almost never part of a URL.
            let mut url = &rest[..end_rel];
            while let Some(last) = url.chars().last() {
                if ".,;:!?)]}’\"'".contains(last) {
                    url = &url[..url.len() - last.len_utf8()];
                } else {
                    break;
                }
            }
            if url.len() <= "https://".len() {
                // Not actually a URL (e.g. bare scheme); fall through.
                out.push_str(&rest[..end_rel]);
                i += end_rel;
                continue;
            }
            let escaped = html_escape_into_string(url);
            out.push_str("<a href=\"");
            out.push_str(&escaped);
            out.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\">");
            out.push_str(&escaped);
            out.push_str("</a>");
            i += url.len();
            continue;
        }
        let ch = line[i..].chars().next().unwrap();
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
        i += ch.len_utf8();
    }
    out
}

fn html_escape_into_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn html_to_text(html: &str) -> String {
    // Strip tags for snippets; insert spacing at block boundaries so words
    // don't run together. Entity decoding is intentionally minimal.
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' => {
                in_tag = false;
                if matches!(
                    tag.trim_start_matches('/').split_whitespace().next(),
                    Some("br" | "p" | "div" | "tr" | "td" | "li" | "h1" | "h2" | "h3" | "table")
                ) {
                    out.push(' ');
                }
            }
            _ if !in_tag => out.push(ch),
            _ => tag.push(ch),
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_lines_keep_their_marker() {
        let (html, quoted) = to_html("Hello there.\n> quoted reply\n> second line\n");
        assert_eq!(quoted, Some(1));
        assert!(html.contains("&gt; quoted reply"), "{html}");
        assert!(!html.contains("&gt;gt;"));
        assert!(html.contains("sift-quote"));
    }

    #[test]
    fn preserves_indentation_and_blank_lines() {
        let (html, _) = to_html("Indented:\n    one\n    two\n\nEnd\n");
        assert!(html.contains("    one"));
        assert!(html.contains("    two"));
        assert!(html.contains("sift-plain"));
    }

    #[test]
    fn linkifies_without_collapsing_whitespace() {
        let (html, _) = to_html("see https://example.com/a?x=1&y=2, then stop\n");
        assert!(html.contains("href=\"https://example.com/a?x=1&amp;y=2\""));
        assert!(html.contains("</a>, then stop"));
    }

    #[test]
    fn escapes_dangerous_content() {
        let (html, _) = to_html("<script>alert(1)</script> & <b>x</b>");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&amp;"));
    }

    #[test]
    fn detects_original_message_marker() {
        let (html, quoted) = to_html("Hi\n\n-----Original Message-----\nold\n");
        assert_eq!(quoted, Some(2));
        assert!(html.contains("Show quoted text"));
    }
}
