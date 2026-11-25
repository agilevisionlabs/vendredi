use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Syntax(pub [u8; 6]);
impl Default for Syntax {
    fn default() -> Self {
        Self(*b":+.? '")
    }
}
impl Syntax {
    pub fn validate(self) -> Result<()> {
        let b = self.0;
        ensure!(
            b.iter().all(|x| (32..127).contains(x)) && (0..6).all(|i| !b[i + 1..].contains(&b[i])),
            "invalid UNA service characters"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Segment {
    pub tag: String,
    pub elements: Vec<Vec<String>>,
    pub offset: usize,
}
impl Segment {
    pub fn at(&self, field: usize, component: usize) -> &str {
        self.elements
            .get(field)
            .and_then(|e| e.get(component))
            .map(String::as_str)
            .unwrap_or("")
    }
    pub fn new(tag: &str, elements: Vec<Vec<String>>) -> Self {
        Self {
            tag: tag.into(),
            elements,
            offset: 0,
        }
    }
    fn encode(&self, syntax: Syntax, out: &mut Vec<u8>) -> Result<()> {
        let [component, element, _, release, _, end] = syntax.0;
        out.extend(self.tag.as_bytes());
        for field in &self.elements {
            out.push(element);
            for (i, value) in field.iter().enumerate() {
                if i > 0 {
                    out.push(component);
                }
                for c in value.chars() {
                    let byte = u8::try_from(c as u32)?;
                    if [component, element, release, end].contains(&byte) {
                        out.push(release);
                    }
                    out.push(byte);
                }
            }
        }
        out.push(end);
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub offset: usize,
    pub text: String,
}
impl Finding {
    pub fn new(code: &'static str, offset: usize, text: impl Into<String>) -> Self {
        Self {
            code,
            offset,
            text: text.into(),
        }
    }
}

fn segment(raw: &[u8], pos: &mut usize, syntax: Syntax) -> Result<Segment, Finding> {
    let offset = *pos;
    let [component, element, _, release, _, end] = syntax.0;
    let mut value = String::new();
    let mut field = Vec::new();
    let mut fields = Vec::new();
    let mut escaped = false;
    while let Some(&byte) = raw.get(*pos) {
        *pos += 1;
        if escaped {
            value.push(char::from(byte));
            escaped = false;
            continue;
        }
        if byte == release {
            escaped = true;
            continue;
        }
        if byte == component || byte == element || byte == end {
            field.push(std::mem::take(&mut value));
            if byte != component {
                fields.push(std::mem::take(&mut field));
            }
            if byte == end {
                let tag = fields.remove(0);
                if tag.len() != 1
                    || tag[0].len() != 3
                    || !tag[0].bytes().all(|b| b.is_ascii_uppercase())
                {
                    return Err(Finding::new(
                        "segment_tag",
                        offset,
                        "expected a three-letter segment tag",
                    ));
                }
                return Ok(Segment {
                    tag: tag.into_iter().next().unwrap(),
                    elements: fields,
                    offset,
                });
            }
        } else {
            value.push(char::from(byte));
        }
    }
    Err(Finding::new(
        "unterminated",
        offset,
        "missing segment terminator",
    ))
}

#[derive(Clone, Debug, Serialize)]
pub struct Message {
    pub segments: Vec<Segment>,
}
impl Message {
    pub fn header(&self) -> &Segment {
        &self.segments[0]
    }
    pub fn kind(&self) -> &str {
        self.header().at(1, 0)
    }
    /// EDIFACT directory version, such as `D`.
    pub fn directory(&self) -> &str {
        self.header().at(1, 1)
    }
    /// EDIFACT directory release, such as `96A`.
    pub fn release(&self) -> &str {
        self.header().at(1, 2)
    }
    /// The standard-neutral version that rules match, such as `D96A`.
    pub fn version(&self) -> String {
        format!("{}{}", self.directory(), self.release())
    }
    pub fn reference(&self) -> &str {
        self.header().at(0, 0)
    }
    /// The message as document text with the default separators, the form
    /// static documents and expected replies use.
    pub fn text(&self) -> Result<String> {
        let mut out = Vec::new();
        for s in &self.segments {
            s.encode(Syntax::default(), &mut out)?;
        }
        Ok(latin1(&out))
    }
    fn validate(&self, findings: &mut Vec<Finding>) {
        let h = self.header();
        if h.at(0, 0).is_empty()
            || self.kind().is_empty()
            || h.at(1, 1).is_empty()
            || h.at(1, 2).is_empty()
            || h.at(1, 3).is_empty()
        {
            findings.push(Finding::new(
                "message_header",
                h.offset,
                "missing UNH reference or identifier",
            ));
        }
        let last = self.segments.last().unwrap();
        if last.tag != "UNT"
            || last.at(0, 0).parse::<usize>().ok() != Some(self.segments.len())
            || last.at(1, 0) != h.at(0, 0)
        {
            findings.push(Finding::new(
                "message_trailer",
                last.offset,
                "missing or mismatched UNT count/reference",
            ));
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Interchange {
    pub header: Segment,
    pub messages: Vec<Message>,
    pub findings: Vec<Finding>,
    pub syntax: Syntax,
}

#[derive(Default, Debug, Serialize)]
pub struct Parsed {
    pub interchanges: Vec<Interchange>,
    pub findings: Vec<Finding>,
}

fn skip_lines(raw: &[u8], pos: &mut usize) {
    while matches!(raw.get(*pos), Some(b'\r' | b'\n')) {
        *pos += 1;
    }
}
fn validate_segment_bytes(
    raw: &[u8],
    start: usize,
    end: usize,
    charset: &str,
    findings: &mut Vec<Finding>,
) {
    if let Some(index) = raw[start..end].iter().position(|&b| match charset {
        "UNOA" => !(b.is_ascii_uppercase() || b.is_ascii_digit() || b" .,-()/='+?:".contains(&b)),
        "UNOB" => !(32..=126).contains(&b),
        "UNOC" => b < 32 || (127..160).contains(&b),
        _ => false,
    }) {
        findings.push(Finding::new(
            "character_set",
            start + index,
            format!("byte is outside the configured {charset} repertoire"),
        ));
    }
}

pub fn parse(raw: &[u8]) -> Parsed {
    let mut out = Parsed::default();
    let mut pos = 0;
    let mut syntax = Syntax::default();
    let mut current: Option<Interchange> = None;
    let mut message: Option<Message> = None;
    while pos < raw.len() {
        skip_lines(raw, &mut pos);
        if pos == raw.len() {
            break;
        }
        if raw[pos..].starts_with(b"UNA") {
            if let Some(bytes) = raw.get(pos + 3..pos + 9) {
                let supplied = Syntax(bytes.try_into().unwrap());
                if supplied.validate().is_ok() {
                    syntax = supplied;
                    pos += 9;
                    continue;
                }
            }
            out.findings
                .push(Finding::new("service_advice", pos, "invalid UNA"));
        }
        let s = match segment(raw, &mut pos, syntax) {
            Ok(s) => s,
            Err(e) => {
                if let Some(i) = &mut current {
                    i.findings.push(e);
                } else {
                    out.findings.push(e);
                }
                continue;
            }
        };
        if s.tag == "UNB" {
            if let Some(mut prev) = current.take() {
                prev.findings.push(Finding::new(
                    "missing_unz",
                    s.offset,
                    "missing UNZ before next interchange",
                ));
                out.interchanges.push(prev);
            }
            message = None;
            let mut findings = Vec::new();
            if !matches!(s.at(0, 0), "UNOA" | "UNOB" | "UNOC")
                || s.at(0, 1) != "3"
                || (1..=4).any(|n| s.at(n, 0).is_empty())
            {
                findings.push(Finding::new(
                    "interchange_header",
                    s.offset,
                    "unsupported syntax or incomplete UNB",
                ));
            }
            let datetime = format!("{}{}", s.at(3, 0), s.at(3, 1));
            if datetime.len() != 10
                || chrono::NaiveDateTime::parse_from_str(&datetime, "%y%m%d%H%M").is_err()
            {
                findings.push(Finding::new(
                    "interchange_time",
                    s.offset,
                    "invalid UNB preparation date or time",
                ));
            }
            validate_segment_bytes(raw, s.offset, pos, s.at(0, 0), &mut findings);
            current = Some(Interchange {
                header: s,
                syntax,
                findings,
                messages: Vec::new(),
            });
            continue;
        }
        let Some(i) = &mut current else {
            out.findings
                .push(Finding::new("outside_envelope", s.offset, "expected UNB"));
            continue;
        };
        validate_segment_bytes(raw, s.offset, pos, i.header.at(0, 0), &mut i.findings);
        match s.tag.as_str() {
            "UNH" => {
                if message.is_some() {
                    i.findings.push(Finding::new(
                        "missing_unt",
                        s.offset,
                        "missing UNT before UNH",
                    ));
                }
                message = Some(Message { segments: vec![s] });
            }
            "UNZ" => {
                if message.take().is_some() {
                    i.findings.push(Finding::new(
                        "missing_unt",
                        s.offset,
                        "missing UNT before UNZ",
                    ));
                }
                if i.messages.is_empty()
                    || s.at(0, 0).parse::<usize>().ok() != Some(i.messages.len())
                    || s.at(1, 0) != i.header.at(4, 0)
                {
                    i.findings.push(Finding::new(
                        "interchange_trailer",
                        s.offset,
                        "incorrect UNZ count or reference",
                    ));
                }
                out.interchanges.push(current.take().unwrap());
                syntax = Syntax::default();
            }
            _ if message.is_some() => {
                let last = s.tag == "UNT";
                message.as_mut().unwrap().segments.push(s);
                if last {
                    let m = message.take().unwrap();
                    m.validate(&mut i.findings);
                    i.messages.push(m);
                }
            }
            _ => i.findings.push(Finding::new(
                "outside_message",
                s.offset,
                "expected UNH or UNZ",
            )),
        }
    }
    if let Some(mut i) = current {
        i.findings
            .push(Finding::new("missing_unz", pos, "missing UNZ"));
        out.interchanges.push(i);
    }
    if out.interchanges.is_empty() && out.findings.is_empty() {
        out.findings
            .push(Finding::new("empty", 0, "empty transfer"));
    }
    out
}

/// Keep the wire spelling for display and the decoded components for inspection.
pub fn lines(raw: &[u8]) -> Vec<serde_json::Value> {
    let mut pos = 0;
    let mut syntax = Syntax::default();
    let mut lines = Vec::new();
    while pos < raw.len() {
        skip_lines(raw, &mut pos);
        if pos == raw.len() {
            break;
        }
        let start = pos;
        if raw[pos..].starts_with(b"UNA")
            && let Some(advice) = raw.get(pos + 3..pos + 9)
        {
            let supplied = Syntax(advice.try_into().unwrap());
            if supplied.validate().is_ok() {
                syntax = supplied;
                pos += 9;
                lines.push(serde_json::json!({"tag":"UNA","raw":latin1(&raw[start..pos]),"elements":[[latin1(advice)]]}));
                continue;
            }
        }
        let decoded = segment(raw, &mut pos, syntax);
        let (tag, elements) = match decoded {
            Ok(s) => (s.tag, s.elements),
            Err(_) => (String::new(), Vec::new()),
        };
        if tag == "UNZ" {
            syntax = Syntax::default();
        }
        lines.push(
            serde_json::json!({"tag":tag,"raw":latin1(&raw[start..pos]),"elements":elements}),
        );
    }
    lines
}
pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

pub fn document(raw: &[u8]) -> Result<Message> {
    let mut pos = 0;
    let mut segments = Vec::new();
    loop {
        skip_lines(raw, &mut pos);
        if pos == raw.len() {
            break;
        }
        let s = segment(raw, &mut pos, Syntax::default())
            .map_err(|e| anyhow::anyhow!("{} at byte {}", e.text, e.offset))?;
        segments.push(s);
    }
    ensure!(
        segments.len() >= 2 && segments[0].tag == "UNH" && segments.last().unwrap().tag == "UNT",
        "expected one UNH…UNT message"
    );
    ensure!(
        segments[1..segments.len() - 1]
            .iter()
            .all(|s| !matches!(s.tag.as_str(), "UNB" | "UNH" | "UNZ" | "UNT")),
        "nested envelope in document"
    );
    ensure!(!segments[0].at(1, 0).is_empty(), "missing message type");
    Ok(Message { segments })
}

pub fn script_document(text: &str) -> Result<Message> {
    let bytes = text
        .chars()
        .map(|c| u8::try_from(u32::from(c)))
        .collect::<Result<Vec<_>, _>>()?;
    document(&bytes)
}

pub fn contrl(i: &Interchange, accept: bool) -> Message {
    let h = &i.header;
    let mut uci = vec![vec![h.at(4, 0).into()]];
    uci.extend((1..=2).map(|n| h.elements.get(n).cloned().unwrap_or_default()));
    uci.push(vec![if accept { "7" } else { "4" }.into()]);
    Message {
        segments: vec![
            Segment::new(
                "UNH",
                vec![
                    vec!["1".into()],
                    vec!["CONTRL".into(), "D".into(), "96A".into(), "UN".into()],
                ],
            ),
            Segment::new("UCI", uci),
            Segment::new("UNT", Vec::new()),
        ],
    }
}

pub struct Envelope<'a> {
    pub syntax: Syntax,
    pub charset: &'a str,
    pub sender: &'a [String],
    pub recipient: &'a [String],
    pub reference: &'a str,
    pub time: chrono::DateTime<chrono::Utc>,
    pub wrong_count: bool,
}
pub fn envelope(env: &Envelope<'_>, message: &Message) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    if env.syntax != Syntax::default() {
        out.extend(b"UNA");
        out.extend(env.syntax.0);
    }
    let unb = Segment::new(
        "UNB",
        vec![
            vec![env.charset.into(), "3".into()],
            env.sender.to_vec(),
            env.recipient.to_vec(),
            vec![
                env.time.format("%y%m%d").to_string(),
                env.time.format("%H%M").to_string(),
            ],
            vec![env.reference.into()],
        ],
    );
    unb.encode(env.syntax, &mut out)?;
    let mut head = message.header().clone();
    head.elements[0] = vec!["1".into()];
    head.encode(env.syntax, &mut out)?;
    for s in &message.segments[1..message.segments.len() - 1] {
        s.encode(env.syntax, &mut out)?;
    }
    Segment::new(
        "UNT",
        vec![vec![message.segments.len().to_string()], vec!["1".into()]],
    )
    .encode(env.syntax, &mut out)?;
    Segment::new(
        "UNZ",
        vec![
            vec![if env.wrong_count { "0" } else { "1" }.into()],
            vec![env.reference.into()],
        ],
    )
    .encode(env.syntax, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    const INPUT: &[u8]=b"UNA*;,! ~UNB;UNOC*3;BUY*ZZ;SELL*ZZ;261124*1030;42~UNH;1;ORDERS*D*96A*UN~FTX;A!;B!*C~UNT;3;1~UNZ;1;42~";
    #[test]
    fn custom_service_characters_and_recovery() {
        let mut raw = b"UNB+UNOC:3+B+S+261124:1000+1'UNZ+9+wrong'".to_vec();
        raw.extend(INPUT);
        let parsed = parse(&raw);
        assert_eq!(parsed.interchanges.len(), 2);
        assert!(!parsed.interchanges[0].findings.is_empty());
        let i = &parsed.interchanges[1];
        assert!(i.findings.is_empty());
        assert_eq!(i.messages[0].segments[1].at(0, 0), "A;B*C");
        let reply = envelope(
            &Envelope {
                syntax: i.syntax,
                charset: "UNOC",
                sender: &i.header.elements[2],
                recipient: &i.header.elements[1],
                reference: "99",
                time: chrono::Utc::now(),
                wrong_count: false,
            },
            &contrl(i, true),
        )
        .unwrap();
        let p = parse(&reply);
        let m = &p.interchanges[0].messages[0];
        assert!(p.interchanges[0].findings.is_empty());
        assert_eq!(m.segments[1].at(0, 0), "42");
        assert_eq!(m.kind(), "CONTRL");
    }
    #[test]
    fn repertoire_and_repeated_messages() {
        let raw=b"UNB+UNOA:3+B+S+261124:1000+1'UNH+1+ORDERS:D:96A:UN'FTX+lower'UNT+3+1'UNH+2+ORDERS:D:96A:UN'UNT+2+2'UNZ+2+1'";
        let i = &parse(raw).interchanges[0];
        assert_eq!(i.messages.len(), 2);
        let findings = &i.findings;
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].offset,
            raw.iter().position(|&b| b == b'l').unwrap()
        );
        let m = script_document("UNH+1+ORDRSP:D:96A:UN'FTX+Café'UNT+3+1'").unwrap();
        assert_eq!(m.segments[1].at(0, 0), "Café");
    }
    #[test]
    fn captures_unparseable_bytes() {
        assert_eq!(parse(b"garbage").findings[0].offset, 0);
    }
}
