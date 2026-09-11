//! Tolerant IMAP response parser for exactly the shapes in Appendix H.
//!
//! Why hand-rolled instead of `imap-proto`: real Gmail mailboxes carry raw
//! UTF-8 inside quoted `X-GM-LABELS` values (e.g. `"Büro"`, `"🧾 Receipts"`),
//! which `imap-proto`'s RFC-strict `quoted` parser rejects (`CHAR = %x01-7F`),
//! killing the whole FETCH stream - silent message loss. This parser takes
//! label bytes as-is and lets UTF-8 through, and skips (rather than fails)
//! any attribute it does not understand.
//!
//! Scope is deliberately narrow: untagged `FETCH` (with the Appendix H item
//! set), `LIST` with SPECIAL-USE, `STATUS`, `SEARCH`, `EXISTS`/`EXPUNGE`,
//! `CAPABILITY`, tagged completions with response codes (`APPENDUID`,
//! `HIGHESTMODSEQ`, `UIDVALIDITY`, `UIDNEXT`, …), and `{n}` literals
//! (including `BODY[x]<origin>` partials). Anything else parses as `Other`.
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Untagged(Untagged),
    TaggedOk {
        tag: String,
        code: Option<ResponseCode>,
        text: String,
    },
    TaggedNo {
        tag: String,
        code: Option<ResponseCode>,
        text: String,
    },
    TaggedBad {
        tag: String,
        code: Option<ResponseCode>,
        text: String,
    },
    /// `+ ...` continuation (IDLE entry, APPEND prompt).
    Cont(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Untagged {
    Capability(Vec<String>),
    Flags(Vec<String>),
    Exists(u32),
    Recent(u32),
    Expunge(u32),
    Fetch {
        seq: u32,
        attrs: Vec<FetchAttr>,
    },
    List {
        attrs: Vec<String>,
        delim: Option<String>,
        name: String,
    },
    Status {
        mailbox: String,
        values: std::collections::BTreeMap<String, u64>,
    },
    Search(Vec<u32>),
    UidValidity(u32),
    UidNext(u32),
    HighestModSeq(u64),
    Other {
        kind: String,
        rest: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchAttr {
    Uid(u32),
    Flags(Vec<String>),
    InternalDate(String),
    Rfc822Size(u32),
    ModSeq(u64),
    GmailMsgId(u64),
    GmailThrId(u64),
    GmailLabels(Vec<String>),
    BodyStructure(BodyStruct),
    /// `BODY[<section>]` or `BODY[<section>]<origin>` literal bytes.
    BodySection {
        section: String,
        origin: u32,
        bytes: Vec<u8>,
    },
    /// `BODY[HEADER.FIELDS (...)]` literal bytes.
    HeaderFields(Vec<u8>),
    /// Anything unrecognized, kept verbatim for forward tolerance.
    Raw(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BodyStruct {
    Single {
        mime: String,
        subtype: String,
        params: Vec<(String, String)>,
        id: Option<String>,
        encoding: String,
        size: u32,
        lines: Option<u32>,
        disposition: Option<(String, Vec<(String, String)>)>,
    },
    Multipart {
        parts: Vec<BodyStruct>,
        subtype: String,
        params: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResponseCode {
    AppendUid {
        uidvalidity: u32,
        uid: u32,
    },
    CopyUid {
        uidvalidity: u32,
        src: String,
        dst: String,
    },
    UidValidity(u32),
    UidNext(u32),
    HighestModSeq(u64),
    ReadWrite,
    ReadOnly,
    Alert(String),
    Capabilities(Vec<String>),
    PermanentFlags(Vec<String>),
    Other {
        kind: String,
        rest: String,
    },
}

/// The Gmail identity of one FETCHed message. Thread/message ids are the
/// same integers the REST API renders as lowercase hex.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GmailMeta {
    pub msgid: Option<u64>,
    pub thrid: Option<u64>,
    pub labels: Vec<String>,
    pub modseq: Option<u64>,
}

pub fn gmail_meta(attrs: &[FetchAttr]) -> GmailMeta {
    let mut m = GmailMeta::default();
    for a in attrs {
        match a {
            FetchAttr::GmailMsgId(v) => m.msgid = Some(*v),
            FetchAttr::GmailThrId(v) => m.thrid = Some(*v),
            FetchAttr::GmailLabels(v) => m.labels = v.clone(),
            FetchAttr::ModSeq(v) => m.modseq = Some(*v),
            _ => {}
        }
    }
    m
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    Incomplete,
    Malformed(String),
}

// ---------------------------------------------------------------------------
// cursor
// ---------------------------------------------------------------------------

struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cur { b, pos: 0 }
    }
    fn rest(&self) -> &'a [u8] {
        &self.b[self.pos..]
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }
    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn eat_ws(&mut self) {
        while self.peek() == Some(b' ') {
            self.pos += 1;
        }
    }
    fn eat_crlf(&mut self) -> Result<(), ParseError> {
        if self.rest().starts_with(b"\r\n") {
            self.pos += 2;
            Ok(())
        } else {
            Err(ParseError::Incomplete)
        }
    }
    fn take_while(&mut self, f: impl Fn(u8) -> bool) -> &'a [u8] {
        let s = self.pos;
        while self.peek().map(&f).unwrap_or(false) {
            self.pos += 1;
        }
        &self.b[s..self.pos]
    }
    fn number<T: std::str::FromStr>(&mut self) -> Result<T, ParseError>
    where
        T::Err: std::fmt::Debug,
    {
        let raw = self.take_while(|c| c.is_ascii_digit());
        if raw.is_empty() {
            return Err(ParseError::Malformed("expected number".into()));
        }
        std::str::from_utf8(raw)
            .map_err(|_| ParseError::Malformed("bad number utf8".into()))?
            .parse()
            .map_err(|_| ParseError::Malformed("bad number".into()))
    }
    fn atom(&mut self) -> Result<&'a [u8], ParseError> {
        // Tolerant atom: anything up to space/paren/bracket/quote/CRLF.
        // Deliberately wider than RFC ATOM so server extensions survive.
        let raw = self.take_while(|c| {
            !matches!(
                c,
                b' ' | b'(' | b')' | b'[' | b']' | b'"' | b'\r' | b'\n' | b'{'
            )
        });
        if raw.is_empty() {
            return Err(ParseError::Malformed("expected atom".into()));
        }
        Ok(raw)
    }
    fn quoted_bytes(&mut self) -> Result<Vec<u8>, ParseError> {
        // Tolerant quoted-string: takes ALL bytes (incl. UTF-8 ≥0x80) except
        // an unescaped `"`, honoring backslash escapes. This is the exact
        // place imap-proto fails on real Gmail labels.
        if !self.eat(b'"') {
            return Err(ParseError::Malformed("expected dquote".into()));
        }
        let mut out = vec![];
        loop {
            match self.peek() {
                None => return Err(ParseError::Incomplete),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        None => return Err(ParseError::Incomplete),
                        Some(c) => {
                            out.push(c);
                            self.pos += 1;
                        }
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }
    fn literal(&mut self) -> Result<Vec<u8>, ParseError> {
        // `{n}` or `{n+}` CRLF + n bytes. `{n-}`/partial sends never appear
        // in responses we parse; `{n+}` is accepted identically.
        if !self.eat(b'{') {
            return Err(ParseError::Malformed("expected literal".into()));
        }
        let n: usize = self.number()?;
        if self.peek() == Some(b'+') {
            self.pos += 1;
        }
        if !self.eat(b'}') {
            return Err(ParseError::Malformed("expected }".into()));
        }
        self.eat_crlf()?;
        if self.rest().len() < n {
            return Err(ParseError::Incomplete);
        }
        let out = self.rest()[..n].to_vec();
        self.pos += n;
        Ok(out)
    }
    /// Skip one balanced item: nested parens, quoted strings, literals.
    fn skip_item(&mut self) -> Result<(), ParseError> {
        self.eat_ws();
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                loop {
                    self.eat_ws();
                    match self.peek() {
                        None => return Err(ParseError::Incomplete),
                        Some(b')') => {
                            self.pos += 1;
                            return Ok(());
                        }
                        _ => self.skip_item()?,
                    }
                }
            }
            Some(b'"') => {
                self.quoted_bytes()?;
                Ok(())
            }
            Some(b'{') => {
                self.literal()?;
                Ok(())
            }
            Some(b'[') => {
                // section spec: consume to matching ] (may nest one level)
                let mut depth = 0;
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    match c {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return Ok(());
                            }
                        }
                        b'"' => {
                            self.pos -= 1;
                            self.quoted_bytes()?;
                        }
                        _ => {}
                    }
                    if self.pos >= self.b.len() {
                        return Err(ParseError::Incomplete);
                    }
                }
                Err(ParseError::Incomplete)
            }
            Some(_) => {
                self.atom()?;
                // atom may be followed by [...] (e.g. BODY[1]<0>) - consume.
                if self.peek() == Some(b'[') {
                    self.skip_item()?;
                }
                // ...or trailing <origin> / {literal}
                if self.peek() == Some(b'<') {
                    self.pos += 1;
                    self.take_while(|c| c != b'>');
                    self.eat(b'>');
                }
                if self.peek() == Some(b'{') {
                    self.literal()?;
                }
                Ok(())
            }
            None => Err(ParseError::Incomplete),
        }
    }
}

fn lossy(v: &[u8]) -> String {
    String::from_utf8_lossy(v).into_owned()
}

// ---------------------------------------------------------------------------
// top level: parse one response, return bytes consumed
// ---------------------------------------------------------------------------

/// Parse a single response from the front of `buf`.
/// Returns bytes consumed; `Incomplete` means "feed more bytes".
pub fn parse_response(buf: &[u8]) -> Result<(usize, Response), ParseError> {
    let mut c = Cur::new(buf);
    match c.peek() {
        Some(b'*') => {
            c.pos += 1;
            c.eat(b' ');
            parse_untagged(&mut c).map(|r| (c.pos, r))
        }
        Some(b'+') => {
            c.pos += 1;
            let line = c.take_while(|b| b != b'\r');
            let text = lossy(line);
            c.eat_crlf()?;
            Ok((c.pos, Response::Cont(text)))
        }
        Some(_) => parse_tagged(&mut c),
        None => Err(ParseError::Incomplete),
    }
}

fn parse_tagged(c: &mut Cur) -> Result<(usize, Response), ParseError> {
    let tag = lossy(c.take_while(|b| b != b' ')).to_string();
    c.eat(b' ');
    let word = lossy(c.take_while(|b| b != b' ' && b != b'\r')).to_uppercase();
    c.eat(b' ');
    let (code, text) = parse_code_text(c)?;
    c.eat_crlf()?;
    let pos = c.pos;
    Ok((
        pos,
        match word.as_str() {
            "OK" => Response::TaggedOk { tag, code, text },
            "NO" => Response::TaggedNo { tag, code, text },
            _ => Response::TaggedBad { tag, code, text },
        },
    ))
}

fn parse_code_text(c: &mut Cur) -> Result<(Option<ResponseCode>, String), ParseError> {
    if !c.eat(b'[') {
        let text = lossy(c.take_while(|b| b != b'\r')).trim().to_string();
        return Ok((None, text));
    }
    let kind_raw = c.take_while(|b| b != b' ' && b != b']');
    let kind = lossy(kind_raw).to_uppercase();
    c.eat(b' ');
    let code = match kind.as_str() {
        "APPENDUID" => {
            let v: u32 = c.number()?;
            c.eat(b' ');
            let u: u32 = c.number()?;
            // Ranges (4523:4525) only occur for multi-APPENDs, which Sift
            // never issues; first uid is what callers need.
            if c.peek() == Some(b':') {
                c.pos += 1;
                let _: u32 = c.number().unwrap_or(u);
            }
            Some(ResponseCode::AppendUid {
                uidvalidity: v,
                uid: u,
            })
        }
        "COPYUID" => {
            let v: u32 = c.number()?;
            c.eat(b' ');
            let src = lossy(c.take_while(|b| b != b' ')).to_string();
            c.eat(b' ');
            let dst = lossy(c.take_while(|b| b != b']')).to_string();
            Some(ResponseCode::CopyUid {
                uidvalidity: v,
                src,
                dst,
            })
        }
        "UIDVALIDITY" => Some(ResponseCode::UidValidity(c.number()?)),
        "UIDNEXT" => Some(ResponseCode::UidNext(c.number()?)),
        "HIGHESTMODSEQ" => Some(ResponseCode::HighestModSeq(c.number()?)),
        "READ-WRITE" => Some(ResponseCode::ReadWrite),
        "READ-ONLY" => Some(ResponseCode::ReadOnly),
        "ALERT" => {
            let t = lossy(c.take_while(|b| b != b']')).to_string();
            Some(ResponseCode::Alert(t))
        }
        _ => {
            let rest = lossy(c.take_while(|b| b != b']')).to_string();
            Some(ResponseCode::Other { kind, rest })
        }
    };
    c.eat(b']');
    c.eat(b' ');
    let text = lossy(c.take_while(|b| b != b'\r')).trim().to_string();
    Ok((code, text))
}

fn parse_untagged(c: &mut Cur) -> Result<Response, ParseError> {
    // numeric-led: seq FETCH / EXISTS / RECENT / EXPUNGE
    let save = c.pos;
    let digits = c.take_while(|b| b.is_ascii_digit());
    if !digits.is_empty() && c.peek() == Some(b' ') {
        c.pos += 1;
        let n: u32 = lossy(digits).parse().unwrap_or(0);
        let word = lossy(c.take_while(|b| b.is_ascii_alphabetic())).to_uppercase();
        match word.as_str() {
            "EXISTS" => {
                c.eat_crlf()?;
                return Ok(Response::Untagged(Untagged::Exists(n)));
            }
            "RECENT" => {
                c.eat_crlf()?;
                return Ok(Response::Untagged(Untagged::Recent(n)));
            }
            "EXPUNGE" => {
                c.eat_crlf()?;
                return Ok(Response::Untagged(Untagged::Expunge(n)));
            }
            "FETCH" => {
                c.eat(b' ');
                let attrs = parse_fetch_attrs(c)?;
                c.eat_crlf()?;
                return Ok(Response::Untagged(Untagged::Fetch { seq: n, attrs }));
            }
            _ => {}
        }
    }
    c.pos = save;
    let word = lossy(c.take_while(|b| b.is_ascii_alphabetic() && b != b' ')).to_uppercase();
    // word stopped at space (or non-alpha like -); re-read full token
    c.pos = save;
    let kind = lossy(c.take_while(|b| b != b' ' && b != b'\r')).to_uppercase();
    let _ = word;
    match kind.as_str() {
        "FLAGS" => {
            c.eat(b' ');
            let flags = parse_paren_list(c)?;
            c.eat_crlf()?;
            Ok(Response::Untagged(Untagged::Flags(flags)))
        }
        "CAPABILITY" => {
            let mut caps = vec![];
            loop {
                c.eat(b' ');
                if c.peek() == Some(b'\r') || c.peek().is_none() {
                    break;
                }
                caps.push(lossy(c.atom()?).to_uppercase());
            }
            c.eat_crlf()?;
            Ok(Response::Untagged(Untagged::Capability(caps)))
        }
        "LIST" => {
            c.eat(b' ');
            let r = parse_list(c)?;
            c.eat_crlf()?;
            Ok(Response::Untagged(Untagged::List {
                attrs: r.0,
                delim: r.1,
                name: r.2,
            }))
        }
        "STATUS" => {
            c.eat(b' ');
            let mbox = if c.peek() == Some(b'"') {
                lossy(&c.quoted_bytes()?)
            } else {
                lossy(c.atom()?).to_string()
            };
            c.eat(b' ');
            if !c.eat(b'(') {
                return Err(ParseError::Malformed("STATUS (".into()));
            }
            let mut values = std::collections::BTreeMap::new();
            loop {
                c.eat_ws();
                if c.eat(b')') {
                    break;
                }
                let k = lossy(c.atom()?).to_uppercase();
                c.eat(b' ');
                let v: u64 = c.number()?;
                values.insert(k, v);
                if c.peek().is_none() {
                    return Err(ParseError::Incomplete);
                }
            }
            c.eat_crlf()?;
            Ok(Response::Untagged(Untagged::Status {
                mailbox: mbox,
                values,
            }))
        }
        "OK" => {
            c.eat(b' ');
            let (code, text) = parse_code_text(c)?;
            c.eat_crlf()?;
            Ok(match code {
                Some(ResponseCode::UidValidity(v)) => Response::Untagged(Untagged::UidValidity(v)),
                Some(ResponseCode::UidNext(v)) => Response::Untagged(Untagged::UidNext(v)),
                Some(ResponseCode::HighestModSeq(v)) => {
                    Response::Untagged(Untagged::HighestModSeq(v))
                }
                _ => Response::Untagged(Untagged::Other {
                    kind: "OK".into(),
                    rest: text,
                }),
            })
        }
        "SEARCH" => {
            let mut uids = vec![];
            loop {
                c.eat(b' ');
                if c.peek() == Some(b'\r') || c.peek().is_none() {
                    break;
                }
                // MODSEQ search data "(MODSEQ n)" may trail; stop there.
                if c.peek() == Some(b'(') {
                    break;
                }
                uids.push(c.number()?);
            }
            // skip any trailing search extensions to EOL (tolerant)
            c.take_while(|b| b != b'\r');
            c.eat_crlf()?;
            Ok(Response::Untagged(Untagged::Search(uids)))
        }
        _ => {
            let rest = lossy(c.take_while(|b| b != b'\r')).to_string();
            c.eat_crlf()?;
            Ok(Response::Untagged(Untagged::Other { kind, rest }))
        }
    }
}

fn parse_paren_list(c: &mut Cur) -> Result<Vec<String>, ParseError> {
    if !c.eat(b'(') {
        return Err(ParseError::Malformed("expected (".into()));
    }
    let mut out = vec![];
    loop {
        c.eat_ws();
        if c.eat(b')') {
            break;
        }
        if c.peek().is_none() {
            return Err(ParseError::Incomplete);
        }
        if c.peek() == Some(b'"') {
            out.push(lossy(&c.quoted_bytes()?));
        } else if c.peek() == Some(b'{') {
            let lit = c.literal()?;
            out.push(lossy(&lit));
        } else {
            out.push(lossy(c.atom()?).to_string());
        }
    }
    Ok(out)
}

fn parse_list(c: &mut Cur) -> Result<(Vec<String>, Option<String>, String), ParseError> {
    if !c.eat(b'(') {
        return Err(ParseError::Malformed("LIST (".into()));
    }
    let mut attrs = vec![];
    loop {
        c.eat_ws();
        if c.eat(b')') {
            break;
        }
        if c.peek() == Some(b'\\') {
            c.pos += 1;
            let name = lossy(c.take_while(|b| b.is_ascii_alphabetic())).to_string();
            attrs.push(format!("\\{name}"));
        } else {
            attrs.push(lossy(c.atom()?).to_string());
        }
        if c.peek().is_none() {
            return Err(ParseError::Incomplete);
        }
    }
    c.eat(b' ');
    // hierarchy delimiter: quoted char or NIL
    let delim = if c.rest().starts_with(b"NIL") {
        c.pos += 3;
        None
    } else if c.peek() == Some(b'"') {
        Some(lossy(&c.quoted_bytes()?))
    } else {
        None
    };
    c.eat(b' ');
    let name = if c.peek() == Some(b'"') {
        lossy(&c.quoted_bytes()?)
    } else if c.peek() == Some(b'{') {
        lossy(&c.literal()?)
    } else {
        lossy(c.take_while(|b| b != b'\r')).trim().to_string()
    };
    Ok((attrs, delim, name))
}

// ---------------------------------------------------------------------------
// FETCH attributes
// ---------------------------------------------------------------------------

fn parse_fetch_attrs(c: &mut Cur) -> Result<Vec<FetchAttr>, ParseError> {
    if !c.eat(b'(') {
        return Err(ParseError::Malformed("FETCH (".into()));
    }
    let mut out = vec![];
    loop {
        c.eat_ws();
        if c.eat(b')') {
            break;
        }
        if c.peek().is_none() {
            return Err(ParseError::Incomplete);
        }
        match parse_fetch_attr(c) {
            Ok(a) => out.push(a),
            Err(ParseError::Incomplete) => return Err(ParseError::Incomplete),
            Err(_) => {
                // Tolerant: keep the raw span and continue with the message.
                let start = c.pos;
                c.skip_item()?;
                out.push(FetchAttr::Raw(lossy(&c.b[start..c.pos])));
            }
        }
    }
    Ok(out)
}

fn parse_fetch_attr(c: &mut Cur) -> Result<FetchAttr, ParseError> {
    let name_raw = c.take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'));
    let name = lossy(name_raw).to_uppercase();
    match name.as_str() {
        "UID" => {
            c.eat(b' ');
            Ok(FetchAttr::Uid(c.number()?))
        }
        "FLAGS" => {
            c.eat(b' ');
            Ok(FetchAttr::Flags(parse_paren_list(c)?))
        }
        "INTERNALDATE" => {
            c.eat(b' ');
            if !c.eat(b'"') {
                return Err(ParseError::Malformed("INTERNALDATE quote".into()));
            }
            let d = lossy(c.take_while(|b| b != b'"')).to_string();
            c.eat(b'"');
            Ok(FetchAttr::InternalDate(d))
        }
        "RFC822.SIZE" => {
            c.eat(b' ');
            Ok(FetchAttr::Rfc822Size(c.number()?))
        }
        "MODSEQ" => {
            c.eat(b' ');
            if !c.eat(b'(') {
                return Err(ParseError::Malformed("MODSEQ (".into()));
            }
            let v: u64 = c.number()?;
            c.eat(b')');
            Ok(FetchAttr::ModSeq(v))
        }
        "X-GM-MSGID" => {
            c.eat(b' ');
            Ok(FetchAttr::GmailMsgId(c.number()?))
        }
        "X-GM-THRID" => {
            c.eat(b' ');
            Ok(FetchAttr::GmailThrId(c.number()?))
        }
        "X-GM-LABELS" => {
            c.eat(b' ');
            Ok(FetchAttr::GmailLabels(parse_gm_labels(c)?))
        }
        "BODYSTRUCTURE" => {
            c.eat(b' ');
            Ok(FetchAttr::BodyStructure(parse_bodystructure(c)?))
        }
        n if n.starts_with("BODY[") || n == "BODY" => parse_body_attr(c, &name),
        _ => Err(ParseError::Malformed(format!("unknown attr {name}"))),
    }
}

/// X-GM-LABELS list: `\`-flags verbatim, quoted/literal UTF-8 verbatim.
/// This is the tolerance imap-proto lacks (it rejects bytes ≥ 0x80).
fn parse_gm_labels(c: &mut Cur) -> Result<Vec<String>, ParseError> {
    if !c.eat(b'(') {
        return Err(ParseError::Malformed("LABELS (".into()));
    }
    let mut out = vec![];
    loop {
        c.eat_ws();
        if c.eat(b')') {
            break;
        }
        if c.peek().is_none() {
            return Err(ParseError::Incomplete);
        }
        if c.peek() == Some(b'"') {
            out.push(lossy(&c.quoted_bytes()?));
        } else if c.peek() == Some(b'{') {
            out.push(lossy(&c.literal()?));
        } else {
            out.push(lossy(c.atom()?).to_string());
        }
    }
    Ok(out)
}

/// `BODY`, `BODY[...]`, `BODY[...]<origin>` (+ optional literal).
/// `name` is always `"BODY"` here (the atom stops at `[`); the bracket part
/// is parsed below. Returns HeaderFields / BodySection.
fn parse_body_attr(c: &mut Cur, _name: &str) -> Result<FetchAttr, ParseError> {
    let mut section = String::new();
    let mut had_brackets = false;
    if c.peek() == Some(b'[') {
        had_brackets = true;
        c.pos += 1;
        // section-spec up to ] (may contain a (...) field list with spaces)
        let mut depth_paren = 0;
        let mut in_q = false;
        let mut raw = vec![];
        loop {
            match c.peek() {
                None => return Err(ParseError::Incomplete),
                Some(b'"') => {
                    in_q = !in_q;
                    raw.push(b'"');
                    c.pos += 1;
                }
                Some(b'(') if !in_q => {
                    depth_paren += 1;
                    raw.push(b'(');
                    c.pos += 1;
                }
                Some(b')') if !in_q && depth_paren > 0 => {
                    depth_paren -= 1;
                    raw.push(b')');
                    c.pos += 1;
                }
                Some(b']') if !in_q && depth_paren == 0 => {
                    c.pos += 1;
                    break;
                }
                Some(ch) => {
                    raw.push(ch);
                    c.pos += 1;
                }
            }
        }
        section = lossy(&raw);
    }
    let mut origin = 0u32;
    if c.peek() == Some(b'<') {
        c.pos += 1;
        origin = c.number()?;
        // `<n>` or `<n.m>`: tolerate a dot-length suffix, only origin matters.
        if c.peek() == Some(b'.') {
            c.pos += 1;
            let _: u32 = c.number().unwrap_or(0);
        }
        if !c.eat(b'>') {
            return Err(ParseError::Malformed("partial >".into()));
        }
    }
    // A literal follows for non-NIL bodies (after optional space).
    c.eat_ws();
    if c.rest().starts_with(b"NIL") {
        c.pos += 3;
        let empty = vec![];
        return Ok(shape_section(had_brackets, &section, origin, empty));
    }
    if c.peek() == Some(b'{') {
        let bytes = c.literal()?;
        return Ok(shape_section(had_brackets, &section, origin, bytes));
    }
    // No data (e.g. empty part with no literal on some servers).
    Ok(shape_section(had_brackets, &section, origin, vec![]))
}

fn shape_section(had_brackets: bool, section: &str, origin: u32, bytes: Vec<u8>) -> FetchAttr {
    if !had_brackets {
        // Bare BODY (RFC2060 body structure without extensions): Gmail never
        // sends it since we always ask BODYSTRUCTURE; keep raw on sight.
        return FetchAttr::Raw(format!("BODY<{origin}>[{} bytes]", bytes.len()));
    }
    if section.is_empty() || section.to_uppercase().starts_with("HEADER") {
        // BODY[] (whole message) and BODY[HEADER...] are header-ish payloads.
        return FetchAttr::HeaderFields(bytes);
    }
    FetchAttr::BodySection {
        section: section.to_string(),
        origin,
        bytes,
    }
}

// ---------------------------------------------------------------------------
// BODYSTRUCTURE
// ---------------------------------------------------------------------------

fn parse_bodystructure(c: &mut Cur) -> Result<BodyStruct, ParseError> {
    if !c.eat(b'(') {
        return Err(ParseError::Malformed("BODYSTRUCTURE (".into()));
    }
    // Multipart starts with another '('; single part starts with a string.
    if c.peek() == Some(b'(') {
        let mut parts = vec![];
        while c.peek() == Some(b'(') {
            parts.push(parse_bodystructure(c)?);
            c.eat_ws();
        }
        c.eat_ws();
        let subtype = parse_nstring_token(c)?;
        c.eat_ws();
        let params = parse_params(c)?;
        // body-ext-mpart: disposition, language, location - skip tolerantly.
        skip_exts(c, 3)?;
        if !c.eat(b')') {
            return Err(ParseError::Malformed("multipart )".into()));
        }
        Ok(BodyStruct::Multipart {
            parts,
            subtype,
            params,
        })
    } else {
        let mime = parse_nstring_token(c)?;
        c.eat_ws();
        let subtype = parse_nstring_token(c)?;
        c.eat_ws();
        let params = parse_params(c)?;
        c.eat_ws();
        let id = parse_nstring_opt(c)?;
        c.eat_ws();
        let _desc = parse_nstring_opt(c)?;
        c.eat_ws();
        let encoding = parse_nstring_token(c)?.to_lowercase();
        c.eat_ws();
        let size: u32 = c.number()?;
        c.eat_ws();
        // text/* has an extra line count
        let mut lines = None;
        if mime.eq_ignore_ascii_case("text")
            && c.peek().map(|b| b.is_ascii_digit()).unwrap_or(false)
        {
            lines = Some(c.number()?);
            c.eat_ws();
        }
        // body-ext-1part: md5, disposition, language, location - disposition
        // matters (attachment filename), the rest is skipped.
        let _md5 = parse_nstring_opt(c)?;
        c.eat_ws();
        let disposition = parse_disposition(c)?;
        c.eat_ws();
        skip_exts(c, 2)?;
        if !c.eat(b')') {
            return Err(ParseError::Malformed("single )".into()));
        }
        Ok(BodyStruct::Single {
            mime,
            subtype,
            params,
            id,
            encoding,
            size,
            lines,
            disposition,
        })
    }
}

/// content-id lives in body-fld-id; surface it once the Single is built.
fn parse_nstring_token(c: &mut Cur) -> Result<String, ParseError> {
    if c.rest().starts_with(b"NIL") {
        c.pos += 3;
        return Ok(String::new());
    }
    if c.peek() == Some(b'"') {
        return Ok(lossy(&c.quoted_bytes()?));
    }
    Ok(lossy(c.atom()?).to_string())
}

fn parse_nstring_opt(c: &mut Cur) -> Result<Option<String>, ParseError> {
    if c.rest().starts_with(b"NIL") {
        c.pos += 3;
        return Ok(None);
    }
    if c.peek() == Some(b'"') {
        return Ok(Some(lossy(&c.quoted_bytes()?)));
    }
    // bare atoms (e.g. unquoted encodings on sloppy servers)
    Ok(Some(lossy(c.atom()?).to_string()))
}

fn parse_params(c: &mut Cur) -> Result<Vec<(String, String)>, ParseError> {
    if c.rest().starts_with(b"NIL") {
        c.pos += 3;
        return Ok(vec![]);
    }
    if !c.eat(b'(') {
        return Err(ParseError::Malformed("params (".into()));
    }
    let mut out = vec![];
    loop {
        c.eat_ws();
        if c.eat(b')') {
            break;
        }
        if c.peek().is_none() {
            return Err(ParseError::Incomplete);
        }
        let k = if c.peek() == Some(b'"') {
            lossy(&c.quoted_bytes()?)
        } else {
            lossy(c.atom()?).to_string()
        };
        c.eat_ws();
        let v = if c.peek() == Some(b'"') {
            lossy(&c.quoted_bytes()?)
        } else if c.peek() == Some(b'{') {
            lossy(&c.literal()?)
        } else {
            lossy(c.atom()?).to_string()
        };
        out.push((k, v));
    }
    Ok(out)
}

type Disposition = (String, Vec<(String, String)>);

fn parse_disposition(c: &mut Cur) -> Result<Option<Disposition>, ParseError> {
    if c.rest().starts_with(b"NIL") {
        c.pos += 3;
        return Ok(None);
    }
    if !c.eat(b'(') {
        // tolerant: bare disposition token
        return Ok(Some((parse_nstring_token(c)?, vec![])));
    }
    c.eat_ws();
    let kind = if c.peek() == Some(b'"') {
        lossy(&c.quoted_bytes()?)
    } else {
        lossy(c.atom()?).to_string()
    };
    c.eat_ws();
    let params = parse_params(c)?;
    c.eat_ws();
    let _ = c.eat(b')');
    Ok(Some((kind, params)))
}

/// Skip up to `n` body-extension items (each NIL | string | (...) ).
fn skip_exts(c: &mut Cur, n: usize) -> Result<(), ParseError> {
    for _ in 0..n {
        if c.peek() == Some(b')') || c.peek().is_none() {
            break;
        }
        c.skip_item()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// misc helpers
// ---------------------------------------------------------------------------

/// Split raw header block bytes into a lowercase-name → value map, folding
/// continuation lines. Used for `BODY[HEADER.FIELDS (...)]` literals.
pub fn parse_header_fields(bytes: &[u8]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut current: Option<String> = None;
    for line in String::from_utf8_lossy(bytes).lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(k) = current.clone() {
                map.entry(k).and_modify(|v| {
                    *v = format!("{} {}", v, line.trim());
                });
            }
        } else if let Some((k, v)) = line.split_once(':') {
            current = Some(k.trim().to_lowercase());
            map.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    map
}

/// INTERNALDATE "12-Aug-2026 09:14:03 +0000" → unix millis.
pub fn parse_internaldate(s: &str) -> Option<i64> {
    if let Ok(d) = chrono::DateTime::parse_from_str(s, "%d-%b-%Y %H:%M:%S %z") {
        return Some(d.timestamp_millis());
    }
    // Fallback without timezone (assume UTC).
    s.rfind(' ').and_then(|i| {
        chrono::NaiveDateTime::parse_from_str(&s[..i], "%d-%b-%Y %H:%M:%S")
            .ok()
            .map(|d| {
                chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(d, chrono::Utc)
                    .timestamp_millis()
            })
    })
}

/// Modified UTF-7 (IMAP mailbox names) → UTF-8. `&` alone means `&`.
pub fn decode_utf7_mailbox(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if rest.starts_with('-') {
            out.push('&');
            rest = &rest[1..];
            continue;
        }
        let end = rest.find('-').unwrap_or(rest.len());
        let b64 = &rest[..end].replace(',', "/");
        rest = if end < rest.len() {
            &rest[end + 1..]
        } else {
            ""
        };
        // modified BASE64 → UTF-16BE → String
        let mut bits: u32 = 0;
        let mut nbits = 0;
        let mut units: Vec<u16> = vec![];
        let mut ok = true;
        for ch in b64.chars() {
            let v = match ch {
                'A'..='Z' => ch as u32 - 'A' as u32,
                'a'..='z' => ch as u32 - 'a' as u32 + 26,
                '0'..='9' => ch as u32 - '0' as u32 + 52,
                '+' => 62,
                '/' => 63,
                _ => {
                    ok = false;
                    break;
                }
            };
            bits = (bits << 6) | v;
            nbits += 6;
            if nbits >= 16 {
                nbits -= 16;
                units.push((bits >> nbits) as u16);
            }
        }
        if ok {
            out.push_str(&String::from_utf16_lossy(&units));
        } else {
            out.push_str(&format!("&{b64}-"));
        }
    }
    out.push_str(rest);
    out
}

/// UTF-8 mailbox name → modified UTF-7 (for SELECT/CREATE arguments).
pub fn encode_utf7_mailbox(s: &str) -> String {
    use base64::Engine;
    let mut out = String::new();
    let mut buf: Vec<u16> = vec![];
    let flush = |out: &mut String, buf: &mut Vec<u16>| {
        if buf.is_empty() {
            return;
        }
        let mut bytes = vec![];
        for u in buf.drain(..) {
            bytes.push((u >> 8) as u8);
            bytes.push((u & 0xFF) as u8);
        }
        let enc = base64::engine::general_purpose::STANDARD
            .encode(&bytes)
            .replace('/', ",");
        let enc = enc.trim_end_matches('=');
        out.push('&');
        out.push_str(enc);
        out.push('-');
    };
    for ch in s.chars() {
        let c = ch as u32;
        if (0x20..=0x7E).contains(&c) {
            flush(&mut out, &mut buf);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            let mut tmp = [0u16; 2];
            for u in ch.encode_utf16(&mut tmp) {
                buf.push(*u);
            }
        }
    }
    flush(&mut out, &mut buf);
    out
}

/// Walk a BODYSTRUCTURE assigning IMAP part numbers ("1", "2", "1.1"...).
/// Returns (number, is_multipart, mime, single-ref) for every addressable part.
pub fn walk_parts(bs: &BodyStruct) -> Vec<(String, &BodyStruct)> {
    fn go<'a>(bs: &'a BodyStruct, prefix: String, out: &mut Vec<(String, &'a BodyStruct)>) {
        match bs {
            BodyStruct::Multipart { parts, .. } => {
                for (i, p) in parts.iter().enumerate() {
                    let n = if prefix.is_empty() {
                        format!("{}", i + 1)
                    } else {
                        format!("{}.{}", prefix, i + 1)
                    };
                    out.push((n.clone(), p));
                    if matches!(p, BodyStruct::Multipart { .. }) {
                        go(p, n, out);
                    }
                }
            }
            single => out.push((prefix, single)),
        }
    }
    let mut out = vec![];
    match bs {
        BodyStruct::Multipart { .. } => go(bs, String::new(), &mut out),
        single => out.push((String::new(), single)),
    }
    out
}

/// Parse a whole transcript file into responses (test/fixture helper; the
/// live session in conn.rs frames incrementally instead).
pub fn parse_transcript(bytes: &[u8]) -> Result<Vec<Response>, ParseError> {
    let mut rest = bytes;
    let mut out = vec![];
    while !rest.iter().all(|b| *b == b'\r' || *b == b'\n') {
        let (used, r) = parse_response(rest)?;
        out.push(r);
        rest = &rest[used..];
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/../fixtures/imap/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(path).unwrap()
    }

    #[test]
    fn p11_t01_fetch_meta_snapshot() {
        let rs = parse_transcript(&fixture("fetch-meta.txt")).unwrap();
        let fetches: Vec<_> = rs
            .iter()
            .filter_map(|r| match r {
                Response::Untagged(Untagged::Fetch { seq, attrs }) => Some((*seq, attrs)),
                _ => None,
            })
            .collect();
        assert_eq!(fetches.len(), 2);
        let m1 = gmail_meta(fetches[0].1);
        assert_eq!(m1.msgid, Some(1912345678901234567));
        assert_eq!(m1.thrid, Some(1912345678901234567));
        assert_eq!(m1.modseq, Some(89123));
        assert!(m1.labels.contains(&"\\Inbox".to_string()));
        assert!(m1.labels.contains(&"Weekly Reports".to_string()));
        assert!(m1.labels.contains(&"Clients/Acme".to_string()));
        let m2 = gmail_meta(fetches[1].1);
        assert_eq!(m2.thrid, Some(1912345678901234001));
        assert!(m2.labels.contains(&"Büro".to_string()));
        assert!(m2.labels.iter().any(|l| l.contains("Receipts")));
        insta::assert_debug_snapshot!(m1);
        insta::assert_debug_snapshot!(m2);
        // bodystructure shape of the mixed message
        let bs = fetches[1]
            .1
            .iter()
            .find_map(|a| match a {
                FetchAttr::BodyStructure(b) => Some(b),
                _ => None,
            })
            .expect("bodystructure");
        insta::assert_debug_snapshot!(bs);
        let parts = walk_parts(bs);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].0, "1");
        assert_eq!(parts[1].0, "2");
        assert_eq!(parts[2].0, "3");
    }

    #[test]
    fn p11_t01_fetch_partial_snapshot() {
        let rs = parse_transcript(&fixture("fetch-partial.txt")).unwrap();
        assert_eq!(rs.len(), 1);
        match &rs[0] {
            Response::Untagged(Untagged::Fetch { attrs, .. }) => {
                let sec = attrs.iter().find_map(|a| match a {
                    FetchAttr::BodySection {
                        section,
                        origin,
                        bytes,
                    } => Some((section.clone(), *origin, bytes.len())),
                    _ => None,
                });
                assert_eq!(sec, Some(("1".to_string(), 0, 2048)));
                insta::assert_debug_snapshot!(gmail_meta(attrs));
            }
            other => panic!("want FETCH, got {other:?}"),
        }
    }

    #[test]
    fn p11_t01_list_special_use_snapshot() {
        let rs = parse_transcript(&fixture("list-special.txt")).unwrap();
        let lists: Vec<_> = rs
            .iter()
            .filter_map(|r| match r {
                Response::Untagged(Untagged::List { attrs, name, .. }) => {
                    Some((attrs.clone(), name.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(lists.len(), 9);
        let all = lists
            .iter()
            .find(|(a, _)| a.iter().any(|x| x == "\\All"))
            .expect("\\All entry");
        assert_eq!(all.1, "[Gmail]/Alle Nachrichten");
        // UTF-7 folder names decode; roles never come from names.
        assert_eq!(
            decode_utf7_mailbox("[Gmail]/Entw&APw-rfe"),
            "[Gmail]/Entwürfe"
        );
        assert_eq!(decode_utf7_mailbox("B&APw-ro"), "Büro");
        assert_eq!(decode_utf7_mailbox("INBOX"), "INBOX");
        assert_eq!(encode_utf7_mailbox("Büro"), "B&APw-ro");
        insta::assert_debug_snapshot!(lists);
    }

    #[test]
    fn p11_t01_select_status_snapshot() {
        let rs = parse_transcript(&fixture("select-status.txt")).unwrap();
        insta::assert_debug_snapshot!(rs);
        assert!(rs.iter().any(|r| matches!(
            r,
            Response::Untagged(Untagged::Search(v)) if v == &[1, 2, 3, 4521, 4522]
        )));
    }

    #[test]
    fn p11_t01_header_fields_and_dates() {
        let h = parse_header_fields(
            b"From: Ada <ada@x.com>\r\nTo: Ben <ben@y.org>,\r\n\tcara@z.org\r\nSubject: Hi\r\n\r\n",
        );
        assert_eq!(h.get("from").map(String::as_str), Some("Ada <ada@x.com>"));
        assert_eq!(
            h.get("to").map(String::as_str),
            Some("Ben <ben@y.org>, cara@z.org")
        );
        assert_eq!(
            parse_internaldate("12-Aug-2026 09:14:03 +0000"),
            Some(1786526043000)
        );
    }

    #[test]
    fn p11_t02_ids_hex_roundtrip() {
        // REST id == lowercase hex of the IMAP decimal (verified 2026-09-04).
        // REST id == lowercase hex of the IMAP decimal, both directions.
        for (dec, hex) in [
            (1912345678901234567u64, "1a8a04434ea94b87"),
            (16u64, "10"),
            (1u64, "1"),
        ] {
            assert_eq!(format!("{dec:x}"), hex);
            assert_eq!(u64::from_str_radix(hex, 16).unwrap(), dec);
        }
    }
}
