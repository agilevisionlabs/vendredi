//! ANSI ASC X12 interchanges: parsing, writing, enveloping and 997s.
//!
//! Delimiters come from the fixed-width `ISA` segment: the element separator
//! is its fourth byte, the repetition separator is `ISA11` (from 00402), the
//! component separator is `ISA16` and the segment terminator follows it.
//! X12 has no release character, so data must not contain a delimiter.
use crate::edi::{Finding, latin1};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

/// The length of an `ISA` segment including its terminator.
pub const ISA_LENGTH: usize = 106;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delimiters {
    pub element: u8,
    /// `None` before version 00402, where `ISA11` is the standards identifier.
    pub repetition: Option<u8>,
    pub component: u8,
    pub segment: u8,
}
impl Default for Delimiters {
    fn default() -> Self {
        Self {
            element: b'*',
            repetition: Some(b'^'),
            component: b':',
            segment: b'~',
        }
    }
}
impl Delimiters {
    fn all(&self) -> impl Iterator<Item = u8> + '_ {
        [self.element, self.component, self.segment]
            .into_iter()
            .chain(self.repetition)
    }
    pub fn validate(&self) -> Result<()> {
        let all: Vec<u8> = self.all().collect();
        ensure!(
            all.iter().all(|b| !b.is_ascii_alphanumeric() && *b != b' ')
                && (0..all.len()).all(|i| !all[i + 1..].contains(&all[i])),
            "X12 delimiters must be distinct and not letters, digits or spaces"
        );
        Ok(())
    }
}

/// One element: its repetitions, each a list of components.
pub type Element = Vec<Vec<String>>;

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub tag: String,
    pub elements: Vec<Element>,
    pub offset: usize,
}
impl Segment {
    pub fn new(tag: &str, elements: Vec<Element>) -> Self {
        Self {
            tag: tag.into(),
            elements,
            offset: 0,
        }
    }
    /// A segment of simple elements.
    pub fn simple(tag: &str, values: &[&str]) -> Self {
        Self::new(
            tag,
            values.iter().map(|v| vec![vec![(*v).to_owned()]]).collect(),
        )
    }
    /// The first component of the first repetition of element `n` (1-based,
    /// as X12 numbers them), or an empty string.
    pub fn at(&self, n: usize) -> &str {
        self.elements
            .get(n.wrapping_sub(1))
            .and_then(|e| e.first())
            .and_then(|c| c.first())
            .map_or("", String::as_str)
    }
    fn encode(&self, d: &Delimiters, out: &mut Vec<u8>) -> Result<()> {
        out.extend(self.tag.as_bytes());
        for (n, element) in self.elements.iter().enumerate() {
            out.push(d.element);
            // ISA11 and ISA16 hold the delimiters themselves.
            if self.tag == "ISA" && (n == 10 || n == 15) {
                let value = element
                    .first()
                    .and_then(|r| r.first())
                    .map_or("", String::as_str);
                out.extend(value.bytes());
                continue;
            }
            for (r, repetition) in element.iter().enumerate() {
                if r > 0 {
                    out.push(d.repetition.ok_or_else(|| {
                        anyhow::anyhow!(
                            "{}: repeated element without a repetition separator",
                            self.tag
                        )
                    })?);
                }
                for (c, value) in repetition.iter().enumerate() {
                    if c > 0 {
                        out.push(d.component);
                    }
                    for ch in value.chars() {
                        let byte = u8::try_from(u32::from(ch)).map_err(|_| {
                            anyhow::anyhow!("{}: {value:?} is not Latin-1", self.tag)
                        })?;
                        ensure!(
                            !d.all().any(|x| x == byte),
                            "{}: {value:?} contains a delimiter, which X12 cannot escape",
                            self.tag
                        );
                        out.push(byte);
                    }
                }
            }
        }
        out.push(d.segment);
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub segments: Vec<Segment>,
}
impl Transaction {
    pub fn header(&self) -> &Segment {
        &self.segments[0]
    }
    /// The transaction set identifier, such as `850`.
    pub fn kind(&self) -> &str {
        self.header().at(1)
    }
    pub fn reference(&self) -> &str {
        self.header().at(2)
    }
    /// The body between `ST` and `SE`.
    pub fn body(&self) -> &[Segment] {
        let end = if self.segments.len() > 1 && self.segments.last().unwrap().tag == "SE" {
            self.segments.len() - 1
        } else {
            self.segments.len()
        };
        &self.segments[1..end.max(1)]
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub header: Segment,
    pub transactions: Vec<Transaction>,
    pub trailer: Option<Segment>,
}
impl Group {
    /// `GS08`, such as `005010`.
    pub fn version(&self) -> &str {
        self.header.at(8)
    }
}

#[derive(Clone, Debug)]
pub struct Interchange {
    pub header: Segment,
    /// Interchange acknowledgements (`TA1`) outside any group.
    pub ta1: Vec<Segment>,
    pub groups: Vec<Group>,
    pub trailer: Option<Segment>,
    pub delimiters: Delimiters,
    /// Bytes written after each segment terminator, such as `\r\n`.
    pub line_ending: Vec<u8>,
    pub findings: Vec<Finding>,
}
impl Interchange {
    /// `ISA13`.
    pub fn control(&self) -> &str {
        self.header.at(13)
    }
}

#[derive(Default, Debug)]
pub struct Parsed {
    pub interchanges: Vec<Interchange>,
    pub findings: Vec<Finding>,
}

/// Reads the delimiters from an `ISA` segment starting at `raw[0]`.
pub fn delimiters(raw: &[u8]) -> Result<Delimiters, Finding> {
    let isa = raw
        .get(..ISA_LENGTH)
        .filter(|isa| isa.starts_with(b"ISA"))
        .ok_or_else(|| Finding::new("isa_length", 0, "ISA is shorter than 106 bytes"))?;
    let element = isa[3];
    // ISA elements have fixed widths, so separators sit at fixed offsets.
    let positions = [6, 17, 20, 31, 34, 50, 53, 69, 76, 81, 83, 89, 99, 101, 103];
    if let Some(&p) = positions.iter().find(|&&p| isa[p] != element) {
        return Err(Finding::new(
            "isa_length",
            p,
            "ISA elements do not have their fixed widths",
        ));
    }
    let version = latin1(&isa[84..89]);
    let repetition = (version.as_str() >= "00402").then_some(isa[82]);
    let d = Delimiters {
        element,
        repetition,
        component: isa[104],
        segment: isa[105],
    };
    d.validate()
        .map_err(|e| Finding::new("delimiters", 0, e.to_string()))?;
    Ok(d)
}

fn segment(raw: &[u8], pos: &mut usize, d: &Delimiters) -> Result<Segment, Finding> {
    let offset = *pos;
    let end = raw[offset..]
        .iter()
        .position(|&b| b == d.segment)
        .map(|n| offset + n)
        .ok_or_else(|| Finding::new("unterminated", offset, "missing segment terminator"))?;
    *pos = end + 1;
    let text = &raw[offset..end];
    let mut fields = text.split(|&b| b == d.element);
    let tag = latin1(fields.next().unwrap_or_default());
    if !(2..=3).contains(&tag.len())
        || !tag
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(Finding::new(
            "segment_tag",
            offset,
            "expected a two- or three-character segment tag",
        ));
    }
    let isa = tag == "ISA";
    let elements = fields
        .enumerate()
        .map(|(i, field)| {
            // ISA11 and ISA16 hold delimiters themselves.
            if isa && (i == 10 || i == 15) {
                return vec![vec![latin1(field)]];
            }
            let repetitions: Vec<&[u8]> = match d.repetition {
                Some(r) => field.split(|&b| b == r).collect(),
                None => vec![field],
            };
            repetitions
                .into_iter()
                .map(|r| r.split(|&b| b == d.component).map(latin1).collect())
                .collect()
        })
        .collect();
    Ok(Segment {
        tag,
        elements,
        offset,
    })
}

fn skip_line_ending(raw: &[u8], pos: &mut usize) -> Vec<u8> {
    let start = *pos;
    while matches!(raw.get(*pos), Some(b'\r' | b'\n')) {
        *pos += 1;
    }
    raw[start..*pos].to_vec()
}

fn count(s: &Segment, n: usize) -> Option<usize> {
    s.at(n).parse().ok()
}

/// Parses every interchange in a transfer, reporting envelope, count and
/// control-number problems with byte offsets.
pub fn parse(raw: &[u8]) -> Parsed {
    let mut out = Parsed::default();
    let mut pos = 0;
    skip_line_ending(raw, &mut pos);
    while pos < raw.len() {
        let start = pos;
        let d = match delimiters(&raw[start..]) {
            Ok(d) => d,
            Err(mut f) => {
                f.offset += start;
                out.findings.push(f);
                return out;
            }
        };
        let header = match segment(raw, &mut pos, &d) {
            Ok(s) => s,
            Err(f) => {
                out.findings.push(f);
                return out;
            }
        };
        let line_ending = skip_line_ending(raw, &mut pos);
        let mut i = Interchange {
            header,
            ta1: Vec::new(),
            groups: Vec::new(),
            trailer: None,
            delimiters: d,
            line_ending,
            findings: Vec::new(),
        };
        let mut group: Option<Group> = None;
        let mut transaction: Option<Transaction> = None;
        while pos < raw.len() && i.trailer.is_none() {
            let s = match segment(raw, &mut pos, &d) {
                Ok(s) => s,
                Err(f) => {
                    i.findings.push(f);
                    pos = raw.len();
                    break;
                }
            };
            skip_line_ending(raw, &mut pos);
            match s.tag.as_str() {
                "ISA" => {
                    i.findings.push(Finding::new(
                        "missing_iea",
                        s.offset,
                        "missing IEA before ISA",
                    ));
                    pos = s.offset;
                    break;
                }
                "GS" => {
                    if let Some(t) = transaction.take() {
                        i.findings.push(Finding::new(
                            "missing_se",
                            s.offset,
                            "missing SE before GS",
                        ));
                        group.as_mut().unwrap().transactions.push(t);
                    }
                    if let Some(g) = group.take() {
                        i.findings.push(Finding::new(
                            "missing_ge",
                            s.offset,
                            "missing GE before GS",
                        ));
                        i.groups.push(g);
                    }
                    group = Some(Group {
                        header: s,
                        transactions: Vec::new(),
                        trailer: None,
                    });
                }
                "ST" => {
                    let Some(g) = &mut group else {
                        i.findings.push(Finding::new(
                            "outside_group",
                            s.offset,
                            "ST outside a GS group",
                        ));
                        continue;
                    };
                    if let Some(t) = transaction.take() {
                        i.findings.push(Finding::new(
                            "missing_se",
                            s.offset,
                            "missing SE before ST",
                        ));
                        g.transactions.push(t);
                    }
                    transaction = Some(Transaction { segments: vec![s] });
                }
                "SE" => {
                    let Some(mut t) = transaction.take() else {
                        i.findings.push(Finding::new(
                            "outside_transaction",
                            s.offset,
                            "SE without ST",
                        ));
                        continue;
                    };
                    let reference = t.reference().to_owned();
                    t.segments.push(s);
                    let se = t.segments.last().unwrap();
                    if count(se, 1) != Some(t.segments.len()) {
                        i.findings.push(Finding::new(
                            "se_count",
                            se.offset,
                            format!(
                                "SE01 counts {} segments, found {}",
                                se.at(1),
                                t.segments.len()
                            ),
                        ));
                    }
                    if se.at(2) != reference {
                        i.findings.push(Finding::new(
                            "se_control",
                            se.offset,
                            format!("SE02 {} does not match ST02 {reference}", se.at(2)),
                        ));
                    }
                    group.as_mut().unwrap().transactions.push(t);
                }
                "GE" => {
                    if let Some(t) = transaction.take() {
                        i.findings.push(Finding::new(
                            "missing_se",
                            s.offset,
                            "missing SE before GE",
                        ));
                        group.as_mut().unwrap().transactions.push(t);
                    }
                    let Some(mut g) = group.take() else {
                        i.findings
                            .push(Finding::new("outside_group", s.offset, "GE without GS"));
                        continue;
                    };
                    if count(&s, 1) != Some(g.transactions.len()) {
                        i.findings.push(Finding::new(
                            "ge_count",
                            s.offset,
                            format!(
                                "GE01 counts {} transaction sets, found {}",
                                s.at(1),
                                g.transactions.len()
                            ),
                        ));
                    }
                    if s.at(2) != g.header.at(6) {
                        i.findings.push(Finding::new(
                            "ge_control",
                            s.offset,
                            format!("GE02 {} does not match GS06 {}", s.at(2), g.header.at(6)),
                        ));
                    }
                    g.trailer = Some(s);
                    i.groups.push(g);
                }
                "IEA" => {
                    if let Some(t) = transaction.take() {
                        i.findings.push(Finding::new(
                            "missing_se",
                            s.offset,
                            "missing SE before IEA",
                        ));
                        group.as_mut().unwrap().transactions.push(t);
                    }
                    if let Some(g) = group.take() {
                        i.findings.push(Finding::new(
                            "missing_ge",
                            s.offset,
                            "missing GE before IEA",
                        ));
                        i.groups.push(g);
                    }
                    if count(&s, 1) != Some(i.groups.len()) {
                        i.findings.push(Finding::new(
                            "iea_count",
                            s.offset,
                            format!("IEA01 counts {} groups, found {}", s.at(1), i.groups.len()),
                        ));
                    }
                    if s.at(2) != i.control() {
                        i.findings.push(Finding::new(
                            "iea_control",
                            s.offset,
                            format!("IEA02 {} does not match ISA13 {}", s.at(2), i.control()),
                        ));
                    }
                    i.trailer = Some(s);
                }
                "TA1" if group.is_none() => i.ta1.push(s),
                _ => match &mut transaction {
                    Some(t) => t.segments.push(s),
                    None => i.findings.push(Finding::new(
                        "outside_transaction",
                        s.offset,
                        format!("{} outside a transaction set", s.tag),
                    )),
                },
            }
        }
        if let Some(t) = transaction.take() {
            i.findings
                .push(Finding::new("missing_se", pos, "missing SE"));
            group
                .get_or_insert_with(|| Group {
                    header: Segment::new("GS", vec![]),
                    transactions: vec![],
                    trailer: None,
                })
                .transactions
                .push(t);
        }
        if let Some(g) = group.take() {
            i.findings
                .push(Finding::new("missing_ge", pos, "missing GE"));
            i.groups.push(g);
        }
        if i.trailer.is_none() && !i.findings.iter().any(|f| f.code == "missing_iea") {
            i.findings
                .push(Finding::new("missing_iea", pos, "missing IEA"));
        }
        out.interchanges.push(i);
    }
    if out.interchanges.is_empty() && out.findings.is_empty() {
        out.findings
            .push(Finding::new("empty", 0, "empty transfer"));
    }
    out
}

/// Writes an interchange back with its own delimiters and line endings.
pub fn write(i: &Interchange) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let put = |s: &Segment, out: &mut Vec<u8>| -> Result<()> {
        s.encode(&i.delimiters, out)?;
        out.extend(&i.line_ending);
        Ok(())
    };
    put(&i.header, &mut out)?;
    for s in &i.ta1 {
        put(s, &mut out)?;
    }
    for g in &i.groups {
        put(&g.header, &mut out)?;
        for t in &g.transactions {
            for s in &t.segments {
                put(s, &mut out)?;
            }
        }
        if let Some(t) = &g.trailer {
            put(t, &mut out)?;
        }
    }
    if let Some(t) = &i.trailer {
        put(t, &mut out)?;
    }
    Ok(out)
}

/// The two-letter `GS01` functional identifier for a transaction set.
pub fn functional_id(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "810" => "IN",
        "820" => "RA",
        "832" => "SC",
        "846" => "IB",
        "850" => "PO",
        "855" => "PR",
        "856" => "SH",
        "860" => "PC",
        "865" => "CA",
        "940" => "OW",
        "945" => "SW",
        "997" | "999" => "FA",
        _ => return None,
    })
}

/// A party in the `ISA` and `GS` segments.
#[derive(Clone, Debug)]
pub struct Party<'a> {
    /// `ISA05`/`ISA07`, such as `ZZ`.
    pub qualifier: &'a str,
    /// `ISA06`/`ISA08`, padded to 15 characters.
    pub id: &'a str,
    /// `GS02`/`GS03`.
    pub application: &'a str,
}

pub struct Envelope<'a> {
    pub delimiters: Delimiters,
    pub sender: Party<'a>,
    pub receiver: Party<'a>,
    /// `ISA13` and `IEA02`, up to nine digits.
    pub control: u64,
    /// `GS06` and `GE02`.
    pub group: u64,
    /// `GS08`, such as `005010` or `004010`. `ISA12` is derived from it.
    pub version: &'a str,
    /// `ISA15`: `T` for test, `P` for production.
    pub usage: char,
    /// `ISA14`: whether a TA1 is requested.
    pub ack_requested: bool,
    pub time: chrono::DateTime<chrono::Utc>,
    pub line_ending: &'a [u8],
    /// Write `IEA01` and `GE01` counts that do not match, for fault simulation.
    pub wrong_count: bool,
}

/// A transaction set to envelope: its identifier, `ST02` and body segments.
pub struct Outgoing<'a> {
    pub kind: &'a str,
    pub reference: &'a str,
    /// `ST03`, the implementation convention reference, when used.
    pub implementation: Option<&'a str>,
    pub body: &'a [Segment],
}

fn fixed(value: &str, width: usize, what: &str) -> Result<String> {
    ensure!(
        value.len() <= width && value.is_ascii(),
        "{what} {value:?} is longer than {width} characters"
    );
    Ok(format!("{value:<width$}"))
}

/// The `ISA` segment of an envelope, padded to its fixed widths.
fn isa(env: &Envelope<'_>) -> Result<Vec<u8>> {
    env.delimiters.validate()?;
    ensure!(
        env.version.len() >= 6 && env.version[..6].bytes().all(|b| b.is_ascii_digit()),
        "expected a GS08 version such as 005010, found {:?}",
        env.version
    );
    ensure!(
        env.control <= 999_999_999,
        "control numbers have at most nine digits"
    );
    ensure!(matches!(env.usage, 'T' | 'P'), "ISA15 is T or P");
    let isa12 = format!("0{}", &env.version[1..5]);
    let modern = isa12.as_str() >= "00402";
    ensure!(
        modern == env.delimiters.repetition.is_some(),
        "version {} {} a repetition separator",
        env.version,
        if modern { "needs" } else { "cannot use" }
    );
    let control = format!("{:09}", env.control);
    let d = env.delimiters;
    // ISA11 and ISA16 are delimiters, written as raw bytes below.
    let isa = [
        "ISA".to_owned(),
        "00".into(),
        " ".repeat(10),
        "00".into(),
        " ".repeat(10),
        fixed(env.sender.qualifier, 2, "ISA05")?,
        fixed(env.sender.id, 15, "ISA06")?,
        fixed(env.receiver.qualifier, 2, "ISA07")?,
        fixed(env.receiver.id, 15, "ISA08")?,
        env.time.format("%y%m%d").to_string(),
        env.time.format("%H%M").to_string(),
        latin1(&[d.repetition.unwrap_or(b'U')]),
        isa12,
        control.clone(),
        if env.ack_requested { "1" } else { "0" }.into(),
        env.usage.to_string(),
        latin1(&[d.component]),
    ];
    let mut out = isa.join(&latin1(&[d.element])).into_bytes();
    out.push(d.segment);
    ensure!(out.len() == ISA_LENGTH, "ISA must be {ISA_LENGTH} bytes");
    out.extend(env.line_ending);
    Ok(out)
}

/// Builds one interchange holding one functional group.
pub fn envelope(env: &Envelope<'_>, sets: &[Outgoing<'_>]) -> Result<Vec<u8>> {
    ensure!(!sets.is_empty(), "an interchange needs a transaction set");
    ensure!(
        env.group <= 999_999_999,
        "control numbers have at most nine digits"
    );
    let kinds: Vec<_> = sets.iter().map(|s| functional_id(s.kind)).collect();
    let gs01 =
        kinds[0].ok_or_else(|| anyhow::anyhow!("unknown transaction set {}", sets[0].kind))?;
    ensure!(
        kinds.iter().all(|k| *k == Some(gs01)),
        "one group holds transaction sets of one functional identifier"
    );
    let control = format!("{:09}", env.control);
    let d = env.delimiters;
    let mut out = isa(env)?;
    let put = |s: Segment, out: &mut Vec<u8>| -> Result<()> {
        s.encode(&d, out)?;
        out.extend(env.line_ending);
        Ok(())
    };
    let group = env.group.to_string();
    put(
        Segment::simple(
            "GS",
            &[
                gs01,
                env.sender.application,
                env.receiver.application,
                &env.time.format("%Y%m%d").to_string(),
                &env.time.format("%H%M").to_string(),
                &group,
                "X",
                env.version,
            ],
        ),
        &mut out,
    )?;
    for set in sets {
        ensure!(
            set.reference.len() >= 4 && set.reference.len() <= 9,
            "ST02 has four to nine characters"
        );
        for s in set.body {
            ensure!(
                !matches!(s.tag.as_str(), "ISA" | "IEA" | "GS" | "GE" | "ST" | "SE"),
                "{} is an envelope segment. The engine writes envelopes",
                s.tag
            );
        }
        let mut st = vec![set.kind, set.reference];
        st.extend(set.implementation);
        put(Segment::simple("ST", &st), &mut out)?;
        for s in set.body {
            put(s.clone(), &mut out)?;
        }
        put(
            Segment::simple("SE", &[&(set.body.len() + 2).to_string(), set.reference]),
            &mut out,
        )?;
    }
    let sets_count = if env.wrong_count { 0 } else { sets.len() };
    put(
        Segment::simple("GE", &[&sets_count.to_string(), &group]),
        &mut out,
    )?;
    let groups = if env.wrong_count { "0" } else { "1" };
    put(Segment::simple("IEA", &[groups, &control]), &mut out)?;
    Ok(out)
}

/// AK5 and AK9 codes for the problems a parse reports.
fn transaction_error(code: &str) -> Option<&'static str> {
    Some(match code {
        "missing_se" => "2",
        "se_control" => "3",
        "se_count" => "4",
        _ => return None,
    })
}

/// The body of a 997 acknowledging one group: `AK1`, an `AK2`/`AK5` pair per
/// transaction set and `AK9`. Findings inside a transaction set reject it.
pub fn acknowledgement(i: &Interchange, group: &Group, reject_all: bool) -> Vec<Segment> {
    let modern = group.version() >= "005010";
    let mut ak1 = vec![group.header.at(1), group.header.at(6)];
    if modern {
        ak1.push(group.version());
    }
    let mut body = vec![Segment::simple("AK1", &ak1)];
    let end = group.trailer.as_ref().map_or(usize::MAX, |t| t.offset);
    let mut accepted = 0;
    for t in &group.transactions {
        let start = t.header().offset;
        let last = t.segments.last().unwrap().offset;
        let errors: Vec<&str> = i
            .findings
            .iter()
            .filter(|f| f.offset >= start && f.offset <= last.max(start))
            .filter_map(|f| transaction_error(f.code))
            .collect();
        body.push(Segment::simple("AK2", &[t.kind(), t.reference()]));
        if errors.is_empty() && !reject_all {
            accepted += 1;
            body.push(Segment::simple("AK5", &["A"]));
        } else {
            let mut ak5 = vec!["R"];
            ak5.extend(errors.iter().take(5));
            if ak5.len() == 1 {
                ak5.push("5");
            }
            body.push(Segment::simple("AK5", &ak5));
        }
    }
    let group_errors: Vec<&str> = i
        .findings
        .iter()
        .filter(|f| group.trailer.is_some() && f.offset == end)
        .filter_map(|f| match f.code {
            "ge_control" => Some("4"),
            "ge_count" => Some("5"),
            _ => None,
        })
        .chain(group.trailer.is_none().then_some("3"))
        .collect();
    let received = group.transactions.len();
    let status = if !group_errors.is_empty() && accepted == 0 {
        "R"
    } else if accepted == received && group_errors.is_empty() {
        "A"
    } else if accepted == 0 {
        "R"
    } else {
        "P"
    };
    let count = group
        .trailer
        .as_ref()
        .map_or(received.to_string(), |t| t.at(1).to_owned());
    let mut ak9 = vec![
        status.to_owned(),
        count,
        received.to_string(),
        accepted.to_string(),
    ];
    ak9.extend(group_errors.iter().take(5).map(|s| (*s).to_owned()));
    body.push(Segment::new(
        "AK9",
        ak9.into_iter().map(|v| vec![vec![v]]).collect(),
    ));
    body
}

/// A static document or expected reply: one `ST`…`SE` transaction set with
/// the default delimiters (`*`, `^`, `:` and `~`).
pub fn document(raw: &[u8]) -> Result<Transaction> {
    let d = Delimiters::default();
    let mut pos = 0;
    let mut segments = Vec::new();
    loop {
        skip_line_ending(raw, &mut pos);
        if pos >= raw.len() {
            break;
        }
        let s = segment(raw, &mut pos, &d)
            .map_err(|f| anyhow::anyhow!("{} at byte {}", f.text, f.offset))?;
        segments.push(s);
    }
    ensure!(
        segments.len() >= 2 && segments[0].tag == "ST" && segments.last().unwrap().tag == "SE",
        "expected one ST…SE transaction set"
    );
    ensure!(
        segments[1..segments.len() - 1]
            .iter()
            .all(|s| !matches!(s.tag.as_str(), "ISA" | "IEA" | "GS" | "GE" | "ST" | "SE")),
        "nested envelope in document"
    );
    ensure!(
        !segments[0].at(1).is_empty(),
        "missing transaction set identifier"
    );
    Ok(Transaction { segments })
}

/// A transaction set as document text with the default delimiters.
pub fn text(segments: &[Segment]) -> Result<String> {
    let mut out = Vec::new();
    for s in segments {
        s.encode(&Delimiters::default(), &mut out)?;
    }
    Ok(latin1(&out))
}

/// Segments for display: the wire text and decoded elements, one entry per
/// component (repetitions are joined back with the repetition separator).
pub fn lines(raw: &[u8]) -> Vec<Value> {
    let mut lines = Vec::new();
    let mut pos = 0;
    skip_line_ending(raw, &mut pos);
    let Ok(d) = delimiters(&raw[pos..]) else {
        return vec![json!({"tag":"","raw":latin1(raw),"elements":[]})];
    };
    while pos < raw.len() {
        let start = pos;
        let decoded = segment(raw, &mut pos, &d);
        if decoded.is_err() {
            // An unterminated or invalid tail is shown as one raw line.
            pos = raw.len();
        }
        let end = pos;
        skip_line_ending(raw, &mut pos);
        let (tag, elements) = match decoded {
            Ok(s) => {
                let rep = latin1(&[d.repetition.unwrap_or(b'^')]);
                let elements: Vec<Vec<String>> = s
                    .elements
                    .iter()
                    .map(|e| match e.as_slice() {
                        [single] => single.clone(),
                        many => vec![
                            many.iter()
                                .map(|r| r.join(&latin1(&[d.component])))
                                .collect::<Vec<_>>()
                                .join(&rep),
                        ],
                    })
                    .collect();
                (s.tag, elements)
            }
            Err(_) => (String::new(), Vec::new()),
        };
        lines.push(json!({"tag":tag,"raw":latin1(&raw[start..end]),"elements":elements}));
    }
    lines
}

/// An interchange holding only a `TA1` segment.
pub fn ta1_interchange(env: &Envelope<'_>, ta1: &Segment) -> Result<Vec<u8>> {
    let mut out = isa(env)?;
    ta1.encode(&env.delimiters, &mut out)?;
    out.extend(env.line_ending);
    let control = format!("{:09}", env.control);
    Segment::simple("IEA", &["0", &control]).encode(&env.delimiters, &mut out)?;
    out.extend(env.line_ending);
    Ok(out)
}

/// The body of a 999 acknowledging one group. It reports the same outcome as
/// the 997, with `IK5` in place of `AK5`.
pub fn implementation_acknowledgement(
    i: &Interchange,
    group: &Group,
    reject_all: bool,
) -> Vec<Segment> {
    acknowledgement(i, group, reject_all)
        .into_iter()
        .map(|mut s| {
            if s.tag == "AK5" {
                s.tag = "IK5".into();
            }
            s
        })
        .collect()
}

fn element(e: &Element) -> Value {
    let repetition = |r: &Vec<String>| match r.as_slice() {
        [single] => json!(single),
        _ => json!(r),
    };
    match e.as_slice() {
        [single] => repetition(single),
        _ => json!({ "repeat": e.iter().map(repetition).collect::<Vec<_>>() }),
    }
}

/// A segment in the script view. Repeated elements become
/// `{"repeat": [...]}`, with each repetition a string or component array.
pub fn segment_view(s: &Segment) -> Value {
    let mut items = vec![json!(s.tag)];
    items.extend(s.elements.iter().map(element));
    Value::Array(items)
}

/// The interchange as scripts see it, in the same shape as EDIFACT. Groups add
/// their `GS` sender, recipient, reference, functional identifier and version.
pub fn view(i: &Interchange) -> Value {
    let party = |q: usize, id: usize| json!([i.header.at(id).trim_end(), i.header.at(q)]);
    json!({
        "format": crate::contract::FORMAT,
        "standard": "x12",
        "sender": party(5, 6),
        "recipient": party(7, 8),
        "reference": i.control(),
        "test": i.header.at(15) == "T",
        "groups": i.groups.iter().map(|g| json!({
            "functional": g.header.at(1),
            "sender": g.header.at(2),
            "recipient": g.header.at(3),
            "reference": g.header.at(6),
            "version": g.version(),
            "messages": g.transactions.iter().map(|t| {
                let mut m = json!({
                    "type": t.kind(),
                    "version": g.version(),
                    "reference": t.reference(),
                    "segments": t.body().iter().map(segment_view).collect::<Vec<_>>(),
                });
                if !t.header().at(3).is_empty() {
                    m["implementation"] = json!(t.header().at(3));
                }
                m
            }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Converts a script view segment back into an X12 segment.
pub fn segment_from_view(items: &[Value]) -> Result<Segment> {
    let text = |v: &Value| -> Result<String> {
        Ok(match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Null => String::new(),
            other => bail!("X12 values are strings or numbers, found {other}"),
        })
    };
    let repetition = |v: &Value| -> Result<Vec<String>> {
        match v {
            Value::Array(c) => c.iter().map(text).collect(),
            other => Ok(vec![text(other)?]),
        }
    };
    let (tag, elements) = items
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty segment"))?;
    let tag = text(tag)?;
    ensure!(
        (2..=3).contains(&tag.len())
            && tag
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()),
        "invalid X12 segment tag {tag:?}"
    );
    let elements = elements
        .iter()
        .map(|e| match e.get("repeat") {
            Some(Value::Array(r)) => r.iter().map(repetition).collect(),
            _ => Ok(vec![repetition(e)?]),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Segment::new(&tag, elements))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIVE: &[u8] = include_bytes!("../../../examples/x12/850-5010.edi");
    const FOUR: &[u8] = include_bytes!("../../../examples/x12/850-4010-custom.edi");

    fn body_text(segments: &[Segment]) -> String {
        let mut out = Vec::new();
        for s in segments {
            s.encode(&Delimiters::default(), &mut out).unwrap();
        }
        latin1(&out)
    }

    #[test]
    fn fixtures_round_trip_byte_for_byte() {
        for raw in [FIVE, FOUR] {
            let parsed = parse(raw);
            assert!(parsed.findings.is_empty(), "{:?}", parsed.findings);
            let i = &parsed.interchanges[0];
            assert!(i.findings.is_empty(), "{:?}", i.findings);
            assert_eq!(write(i).unwrap(), raw);
        }
        let five = &parse(FIVE).interchanges[0];
        assert_eq!(five.delimiters, Delimiters::default());
        assert_eq!(five.line_ending, b"\r\n");
        assert_eq!(five.groups.len(), 2);
        assert_eq!(five.groups[0].transactions.len(), 2);
        let refs = &five.groups[0].transactions[0].segments[2];
        assert_eq!(
            refs.elements[2],
            [vec!["Dock".to_owned()], vec!["Gate 4".to_owned()]]
        );
        let four = &parse(FOUR).interchanges[0];
        assert_eq!(four.delimiters.repetition, None);
        assert_eq!(
            (four.delimiters.element, four.delimiters.segment),
            (b'|', b'!')
        );
        let po1 = &four.groups[0].transactions[0].segments[5];
        assert_eq!(
            po1.elements[8],
            [vec!["WIDGET".to_owned(), "BLUE".to_owned()]]
        );
    }

    #[test]
    fn malformed_envelopes_report_offsets() {
        let text = std::str::from_utf8(FIVE).unwrap();
        let offset = |needle: &str| text.find(needle).unwrap();
        for (broken, code, at) in [
            (
                text.replacen("*BUYER          *", "*BUYER         *", 1),
                "isa_length",
                50,
            ),
            (
                text.replacen("SE*9*0002", "SE*8*0002", 1),
                "se_count",
                offset("SE*9*0002"),
            ),
            (
                text.replacen("SE*9*0003", "SE*9*0004", 1),
                "se_control",
                offset("SE*9*0003"),
            ),
            (
                text.replacen("GE*2*1001", "GE*3*1001", 1),
                "ge_count",
                offset("GE*2*1001"),
            ),
            (
                text.replacen("GE*1*1002", "GE*1*9999", 1),
                "ge_control",
                offset("GE*1*1002"),
            ),
            (
                text.replacen("IEA*2*", "IEA*1*", 1),
                "iea_count",
                offset("IEA*2*"),
            ),
            (
                text.replacen("IEA*2*000004711", "IEA*2*000004712", 1),
                "iea_control",
                offset("IEA*2*"),
            ),
        ] {
            let parsed = parse(broken.as_bytes());
            let findings: Vec<_> = parsed
                .findings
                .iter()
                .chain(parsed.interchanges.iter().flat_map(|i| &i.findings))
                .map(|f| (f.code, f.offset))
                .collect();
            assert_eq!(findings, [(code, at)], "{code}");
        }
    }

    #[test]
    fn envelopes_build_parseable_interchanges() -> Result<()> {
        let source = &parse(FIVE).interchanges[0];
        let t = &source.groups[0].transactions[0];
        let time = "2026-11-25T10:30:00Z".parse()?;
        let env = |delimiters, version, wrong_count| Envelope {
            delimiters,
            sender: Party {
                qualifier: "ZZ",
                id: "SUPPLIER",
                application: "SUPPLIERAPP",
            },
            receiver: Party {
                qualifier: "ZZ",
                id: "BUYER",
                application: "BUYERAPP",
            },
            control: 42,
            group: 7,
            version,
            usage: 'T',
            ack_requested: false,
            time,
            line_ending: b"",
            wrong_count,
        };
        let set = Outgoing {
            kind: "850",
            reference: "0001",
            implementation: None,
            body: t.body(),
        };
        let bytes = envelope(&env(Delimiters::default(), "005010", false), &[set])?;
        let parsed = parse(&bytes);
        let i = &parsed.interchanges[0];
        assert!(i.findings.is_empty(), "{:?}", i.findings);
        assert_eq!(i.control(), "000000042");
        assert_eq!(&bytes[..ISA_LENGTH], b"ISA*00*          *00*          *ZZ*SUPPLIER       *ZZ*BUYER          *261125*1030*^*00501*000000042*0*T*:~");
        assert_eq!(
            body_text(i.groups[0].transactions[0].body()),
            body_text(t.body())
        );
        let custom = Delimiters {
            element: b'|',
            repetition: None,
            component: b'>',
            segment: b'!',
        };
        let set = Outgoing {
            kind: "850",
            reference: "0001",
            implementation: None,
            body: t.body(),
        };
        // The 5010 fixture repeats REF03, which 4010 cannot express.
        assert!(envelope(&env(custom, "004010", false), &[set]).is_err());
        let four = &parse(FOUR).interchanges[0];
        let set = Outgoing {
            kind: "850",
            reference: "0001",
            implementation: None,
            body: four.groups[0].transactions[0].body(),
        };
        let bytes = envelope(&env(custom, "004010", true), &[set])?;
        let codes: Vec<_> = parse(&bytes).interchanges[0]
            .findings
            .iter()
            .map(|f| f.code)
            .collect();
        assert_eq!(codes, ["ge_count", "iea_count"]);
        let bad = [Segment::simple("FTX", &["a*b"])];
        let set = Outgoing {
            kind: "850",
            reference: "0001",
            implementation: None,
            body: &bad,
        };
        assert!(envelope(&env(Delimiters::default(), "005010", false), &[set]).is_err());
        let bad = [Segment::simple("SE", &["1"])];
        let set = Outgoing {
            kind: "850",
            reference: "0001",
            implementation: None,
            body: &bad,
        };
        assert!(envelope(&env(Delimiters::default(), "005010", false), &[set]).is_err());
        Ok(())
    }

    #[test]
    fn acknowledgements_match_reviewed_fixtures() {
        let check = |raw: &[u8], group: usize, reject: bool, expected: &str| {
            let parsed = parse(raw);
            let i = &parsed.interchanges[0];
            let mut body = vec![Segment::simple("ST", &["997", "0001"])];
            body.extend(acknowledgement(i, &i.groups[group], reject));
            body.push(Segment::simple(
                "SE",
                &[&(body.len() + 1).to_string(), "0001"],
            ));
            assert_eq!(body_text(&body), expected.trim_end());
        };
        check(
            FIVE,
            0,
            false,
            include_str!("../../../examples/x12/997-accepted.edi"),
        );
        check(
            FIVE,
            0,
            true,
            include_str!("../../../examples/x12/997-rejected.edi"),
        );
        let partial = std::str::from_utf8(FIVE)
            .unwrap()
            .replacen("SE*9*0002", "SE*8*0002", 1);
        check(
            partial.as_bytes(),
            0,
            false,
            include_str!("../../../examples/x12/997-partial.edi"),
        );
        check(
            FOUR,
            0,
            false,
            include_str!("../../../examples/x12/997-4010-accepted.edi"),
        );
    }

    #[test]
    fn script_view_round_trips_segments() -> Result<()> {
        let i = &parse(FIVE).interchanges[0];
        let v = view(i);
        assert_eq!(v["standard"], "x12");
        assert_eq!(v["sender"], json!(["BUYER", "ZZ"]));
        assert_eq!(v["reference"], "000004711");
        assert_eq!(v["test"], true);
        let g = &v["groups"][1];
        assert_eq!(
            (g["functional"].as_str(), g["reference"].as_str()),
            (Some("PO"), Some("1002"))
        );
        let m = &v["groups"][0]["messages"][1];
        assert_eq!(
            (
                m["type"].as_str(),
                m["version"].as_str(),
                m["reference"].as_str()
            ),
            (Some("850"), Some("005010"), Some("0002"))
        );
        assert_eq!(
            m["segments"][1],
            json!(["REF", "DP", "038", {"repeat": ["Dock", "Gate 4"]}])
        );
        assert_eq!(m["segments"][4][9], json!(["WIDGET", "BLUE"]));
        let body = &i.groups[0].transactions[1].segments[1..9];
        for (view, segment) in m["segments"].as_array().unwrap().iter().zip(body) {
            let back = segment_from_view(view.as_array().unwrap())?;
            assert_eq!(
                (&back.tag, &back.elements),
                (&segment.tag, &segment.elements)
            );
        }
        Ok(())
    }
}
