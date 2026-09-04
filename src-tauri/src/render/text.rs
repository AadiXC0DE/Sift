//! text/plain -> HTML with auto-links, whitespace, quote collapse.

pub fn to_html(text: &str) -> (String, Option<usize>) {
    let mut html = String::from("<div class=\"sift-plain\">");
    let mut quoted_from: Option<usize> = None;
    for (i, line) in text.lines().enumerate() {
        let esc = html_escape(line);
        if line.starts_with('>') {
            if quoted_from.is_none() {
                quoted_from = Some(i);
            }
            html.push_str(&format!("<blockquote>{}</blockquote>", linkify(&esc[1..])));
        } else if line.trim().is_empty() {
            html.push_str("<br>");
        } else if line.trim() == "-----Original Message-----"
            || (line.starts_with("On ") && line.ends_with("wrote:"))
        {
            if quoted_from.is_none() {
                quoted_from = Some(i);
            }
            html.push_str(&format!(
                "<details class=\"sift-quote\"><summary>•••</summary><div>{}</div>",
                linkify(&esc)
            ));
        } else {
            html.push_str(&format!("<div>{}</div>", linkify(&esc)));
        }
    }
    if quoted_from.is_some() {
        html.push_str("</details>");
    }
    html.push_str("</div>");
    (html, quoted_from)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn linkify(s: &str) -> String {
    // naive URL linkify
    let mut out = String::new();
    for tok in s.split_whitespace() {
        if tok.starts_with("http://") || tok.starts_with("https://") {
            out.push_str(&format!(
                "<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"{tok}\">{tok}</a> "
            ));
        } else {
            out.push_str(tok);
            out.push(' ');
        }
    }
    out.trim_end().to_string()
}

pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    // append link URLs in brackets
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
