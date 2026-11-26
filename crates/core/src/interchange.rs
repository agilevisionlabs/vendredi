//! The interface the engine uses for every EDI standard.
//!
//! EDIFACT and X12 differ in envelopes, groups, delimiters and
//! acknowledgements. This module hides those differences behind detection,
//! parsing, message summaries, acknowledgement correlation, the script view,
//! documents and outgoing messages, so the engine handles both the same way.
use crate::{
    config::{GroupAck, Partner, Standard},
    contract,
    edi::{self, Finding},
    x12,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

pub enum Interchange {
    Edifact(edi::Interchange),
    X12(x12::Interchange),
}

#[derive(Default)]
pub struct Parsed {
    pub interchanges: Vec<Interchange>,
    pub findings: Vec<Finding>,
}

/// The standard of a transfer, from its first bytes.
pub fn detect(raw: &[u8]) -> Option<Standard> {
    let start = raw.iter().position(|b| !b.is_ascii_whitespace())?;
    let raw = &raw[start..];
    if raw.starts_with(b"ISA") {
        Some(Standard::X12)
    } else if raw.starts_with(b"UNA") || raw.starts_with(b"UNB") {
        Some(Standard::Edifact)
    } else {
        None
    }
}

/// Parses a transfer in the standard its first bytes name. Unrecognised
/// bytes are reported the way the EDIFACT parser reports them.
pub fn parse(raw: &[u8]) -> Parsed {
    match detect(raw) {
        Some(Standard::X12) => {
            let p = x12::parse(raw);
            Parsed {
                interchanges: p.interchanges.into_iter().map(Interchange::X12).collect(),
                findings: p.findings,
            }
        }
        _ => {
            let p = edi::parse(raw);
            Parsed {
                interchanges: p
                    .interchanges
                    .into_iter()
                    .map(Interchange::Edifact)
                    .collect(),
                findings: p.findings,
            }
        }
    }
}

/// Segments for display, with wire text and decoded elements.
pub fn lines(raw: &[u8]) -> Vec<Value> {
    match detect(raw) {
        Some(Standard::X12) => x12::lines(raw),
        _ => edi::lines(raw),
    }
}

/// What rules match and the traffic list shows for one message.
#[derive(Clone, Debug)]
pub struct MessageInfo {
    /// `ORDERS` or `850`.
    pub kind: String,
    /// `D96A` or the X12 `GS08`, such as `005010`.
    pub version: String,
    /// EDIFACT directory version (`D`), or the X12 version.
    pub directory: String,
    /// EDIFACT release (`96A`). Empty for X12.
    pub release: String,
    pub reference: String,
    /// The X12 `GS06` of the message's group. Empty for EDIFACT.
    pub group: String,
    pub segments: usize,
    /// An X12 transaction set with its own count or control problems. The
    /// group acknowledgement rejects it and no rule runs for it.
    pub rejected: bool,
}

/// Problems that reject one X12 transaction set but not its interchange.
const TRANSACTION_FINDINGS: [&str; 3] = ["missing_se", "se_control", "se_count"];

/// An acknowledgement found in an inbound interchange.
pub struct AckInfo {
    /// The control reference it acknowledges.
    pub reference: String,
    /// `accepted`, `rejected`, `partial` or `unknown`.
    pub status: &'static str,
    /// The identities it names, when the standard carries them.
    pub parties: Option<(Vec<String>, Vec<String>)>,
    /// X12 control numbers compare as numbers; `GS06` drops leading zeros.
    pub numeric: bool,
}

impl Interchange {
    pub fn standard(&self) -> Standard {
        match self {
            Self::Edifact(_) => Standard::Edifact,
            Self::X12(_) => Standard::X12,
        }
    }
    pub fn offset(&self) -> usize {
        match self {
            Self::Edifact(i) => i.header.offset,
            Self::X12(i) => i.header.offset,
        }
    }
    /// `[id, qualifier]`.
    pub fn sender(&self) -> Vec<String> {
        match self {
            Self::Edifact(i) => i.header.elements.get(1).cloned().unwrap_or_default(),
            Self::X12(i) => vec![i.header.at(6).trim_end().into(), i.header.at(5).into()],
        }
    }
    pub fn recipient(&self) -> Vec<String> {
        match self {
            Self::Edifact(i) => i.header.elements.get(2).cloned().unwrap_or_default(),
            Self::X12(i) => vec![i.header.at(8).trim_end().into(), i.header.at(7).into()],
        }
    }
    pub fn reference(&self) -> &str {
        match self {
            Self::Edifact(i) => i.header.at(4, 0),
            Self::X12(i) => i.control(),
        }
    }
    pub fn test_flag(&self) -> &str {
        match self {
            Self::Edifact(i) => i.header.at(10, 0),
            Self::X12(i) => i.header.at(15),
        }
    }
    pub fn ack_requested(&self) -> bool {
        match self {
            Self::Edifact(i) => i.header.at(8, 0) == "1",
            Self::X12(i) => i.header.at(14) == "1",
        }
    }
    /// The EDIFACT syntax identifier, such as `UNOC`.
    pub fn charset(&self) -> Option<&str> {
        match self {
            Self::Edifact(i) => Some(i.header.at(0, 0)),
            Self::X12(_) => None,
        }
    }
    pub fn findings(&self) -> &[Finding] {
        match self {
            Self::Edifact(i) => &i.findings,
            Self::X12(i) => &i.findings,
        }
    }
    pub fn findings_mut(&mut self) -> &mut Vec<Finding> {
        match self {
            Self::Edifact(i) => &mut i.findings,
            Self::X12(i) => &mut i.findings,
        }
    }
    pub fn messages(&self) -> Vec<MessageInfo> {
        match self {
            Self::Edifact(i) => i
                .messages
                .iter()
                .map(|m| MessageInfo {
                    kind: m.kind().into(),
                    version: m.version(),
                    directory: m.directory().into(),
                    release: m.release().into(),
                    reference: m.reference().into(),
                    group: String::new(),
                    segments: m.segments.len(),
                    rejected: false,
                })
                .collect(),
            Self::X12(i) => i
                .groups
                .iter()
                .flat_map(|g| {
                    g.transactions.iter().map(move |t| {
                        let (start, end) = (t.header().offset, t.segments.last().unwrap().offset);
                        MessageInfo {
                            kind: t.kind().into(),
                            version: g.version().into(),
                            directory: g.version().into(),
                            release: String::new(),
                            reference: t.reference().into(),
                            group: g.header.at(6).into(),
                            segments: t.segments.len(),
                            rejected: i.findings.iter().any(|f| {
                                TRANSACTION_FINDINGS.contains(&f.code)
                                    && (start..=end).contains(&f.offset)
                            }),
                        }
                    })
                })
                .collect(),
        }
    }
    /// Whether the interchange can be processed. EDIFACT rejects it for any
    /// finding. X12 rejects only the transaction sets a finding falls in.
    pub fn accepted(&self) -> bool {
        match self {
            Self::Edifact(i) => i.findings.is_empty(),
            Self::X12(i) => i
                .findings
                .iter()
                .all(|f| TRANSACTION_FINDINGS.contains(&f.code)),
        }
    }
    /// The script view from the `contract` module.
    pub fn view(&self) -> Value {
        match self {
            Self::Edifact(i) => contract::interchange(i, Standard::Edifact),
            Self::X12(i) => x12::view(i),
        }
    }
    /// The 0.1 script layout, which only EDIFACT scripts used.
    pub fn legacy(&self) -> Option<&edi::Interchange> {
        match self {
            Self::Edifact(i) => Some(i),
            Self::X12(_) => None,
        }
    }
    /// Whether the interchange carries acknowledgements, which are never
    /// acknowledged themselves.
    pub fn carries_acknowledgement(&self) -> bool {
        match self {
            Self::Edifact(i) => i.messages.iter().any(|m| m.kind() == "CONTRL"),
            Self::X12(i) => {
                !i.ta1.is_empty()
                    || i.groups
                        .iter()
                        .flat_map(|g| &g.transactions)
                        .any(|t| matches!(t.kind(), "997" | "999"))
            }
        }
    }
    pub fn acknowledgements(&self) -> Vec<AckInfo> {
        match self {
            Self::Edifact(i) => i
                .messages
                .iter()
                .filter(|m| m.kind() == "CONTRL")
                .filter_map(|m| m.segments.iter().find(|s| s.tag == "UCI"))
                .map(|uci| AckInfo {
                    reference: uci.at(0, 0).into(),
                    status: match uci.at(3, 0) {
                        "7" | "8" => "accepted",
                        "4" => "rejected",
                        _ => "unknown",
                    },
                    parties: Some((
                        uci.elements.get(1).cloned().unwrap_or_default(),
                        uci.elements.get(2).cloned().unwrap_or_default(),
                    )),
                    numeric: false,
                })
                .collect(),
            Self::X12(i) => {
                let ta1 = i.ta1.iter().map(|s| AckInfo {
                    reference: s.at(1).into(),
                    status: match s.at(4) {
                        "A" | "E" => "accepted",
                        "R" => "rejected",
                        _ => "unknown",
                    },
                    parties: None,
                    numeric: true,
                });
                let groups = i
                    .groups
                    .iter()
                    .flat_map(|g| &g.transactions)
                    .filter(|t| matches!(t.kind(), "997" | "999"))
                    .filter_map(|t| {
                        let ak1 = t.segments.iter().find(|s| s.tag == "AK1")?;
                        let ak9 = t.segments.iter().find(|s| s.tag == "AK9");
                        Some(AckInfo {
                            reference: ak1.at(2).into(),
                            status: match ak9.map_or("", |s| s.at(1)) {
                                "A" | "E" => "accepted",
                                "R" => "rejected",
                                "P" => "partial",
                                _ => "unknown",
                            },
                            parties: None,
                            numeric: true,
                        })
                    });
                ta1.chain(groups).collect()
            }
        }
    }
    /// Each message as document text with the default delimiters, as cases
    /// write expected replies.
    pub fn message_texts(&self) -> Result<Vec<String>> {
        match self {
            Self::Edifact(i) => i.messages.iter().map(|m| m.text()).collect(),
            Self::X12(i) => {
                let mut texts: Vec<String> = i
                    .ta1
                    .iter()
                    .map(|s| x12::text(std::slice::from_ref(s)))
                    .collect::<Result<_>>()?;
                for t in i.groups.iter().flat_map(|g| &g.transactions) {
                    texts.push(x12::text(&t.segments)?);
                }
                Ok(texts)
            }
        }
    }
}

/// An X12 transaction set to send.
#[derive(Clone, Debug)]
pub struct X12Message {
    pub kind: String,
    /// `GS08`. Defaults to the partner's `x12.version`.
    pub version: Option<String>,
    /// `ST03`.
    pub implementation: Option<String>,
    pub body: Vec<x12::Segment>,
}

/// A message the engine envelopes and sends.
#[derive(Clone, Debug)]
pub enum Outgoing {
    Edifact(edi::Message),
    X12(X12Message),
    /// An X12 interchange acknowledgement, sent between `ISA` and `IEA`.
    Ta1(x12::Segment),
}
impl Outgoing {
    pub fn kind(&self) -> &str {
        match self {
            Self::Edifact(m) => m.kind(),
            Self::X12(m) => &m.kind,
            Self::Ta1(_) => "TA1",
        }
    }
    pub fn standard(&self) -> Standard {
        match self {
            Self::Edifact(_) => Standard::Edifact,
            _ => Standard::X12,
        }
    }
    /// The message as document text with the default delimiters.
    pub fn text(&self) -> Result<String> {
        match self {
            Self::Edifact(m) => m.text(),
            Self::X12(m) => {
                let mut st = vec![m.kind.as_str(), "0001"];
                st.extend(m.implementation.as_deref());
                let mut segments = vec![x12::Segment::simple("ST", &st)];
                segments.extend(m.body.iter().cloned());
                let count = (segments.len() + 1).to_string();
                segments.push(x12::Segment::simple("SE", &[&count, "0001"]));
                x12::text(&segments)
            }
            Self::Ta1(s) => x12::text(std::slice::from_ref(s)),
        }
    }
}

/// Reads a static document, expected reply or sample: one `UNH`…`UNT` or
/// `ST`…`SE` message with the standard's default delimiters.
pub fn document(standard: Standard, raw: &[u8]) -> Result<Outgoing> {
    Ok(match standard {
        Standard::Edifact => Outgoing::Edifact(edi::document(raw)?),
        Standard::X12 => {
            let t = x12::document(raw)?;
            let implementation = Some(t.header().at(3).to_owned()).filter(|s| !s.is_empty());
            Outgoing::X12(X12Message {
                kind: t.kind().into(),
                version: None,
                implementation,
                body: t.body().to_vec(),
            })
        }
    })
}

/// The acknowledgements a partner sends for an inbound interchange.
pub fn acknowledge(partner: &Partner, i: &Interchange, accepted: bool) -> Vec<Outgoing> {
    match i {
        Interchange::Edifact(i) => vec![Outgoing::Edifact(edi::contrl(i, accepted))],
        Interchange::X12(i) => {
            let settings = partner.x12();
            let mut out = Vec::new();
            if settings.ta1 {
                out.push(Outgoing::Ta1(x12::Segment::simple(
                    "TA1",
                    &[
                        i.control(),
                        i.header.at(9),
                        i.header.at(10),
                        if accepted { "A" } else { "R" },
                        if accepted { "000" } else { "022" },
                    ],
                )));
            }
            for g in &i.groups {
                let (kind, body) = match settings.acknowledgement {
                    GroupAck::FunctionalAck => ("997", x12::acknowledgement(i, g, !accepted)),
                    GroupAck::ImplementationAck => {
                        ("999", x12::implementation_acknowledgement(i, g, !accepted))
                    }
                };
                out.push(Outgoing::X12(X12Message {
                    kind: kind.into(),
                    version: Some(g.version().to_owned()).filter(|v| !v.is_empty()),
                    implementation: None,
                    body,
                }));
            }
            out
        }
    }
}

/// Who an interchange travels between.
pub struct Route<'a> {
    pub sender: &'a [String],
    pub recipient: &'a [String],
    /// The X12 `GS02` and `GS03` application codes.
    pub applications: (&'a str, &'a str),
}

fn party<'a>(id: &'a [String], application: &'a str) -> x12::Party<'a> {
    x12::Party {
        qualifier: id.get(1).map_or("ZZ", String::as_str),
        id: id.first().map_or("", String::as_str),
        application,
    }
}

/// Envelopes a message for a partner of the matching standard.
pub fn envelope(
    partner: &Partner,
    route: &Route<'_>,
    reference: &str,
    time: chrono::DateTime<chrono::Utc>,
    wrong_count: bool,
    message: &Outgoing,
) -> Result<Vec<u8>> {
    ensure!(
        message.standard() == partner.standard,
        "a {} message cannot be sent to the {} partner {}",
        message.standard().label(),
        partner.standard.label(),
        partner.name
    );
    match message {
        Outgoing::Edifact(m) => edi::envelope(
            &edi::Envelope {
                syntax: partner.syntax()?,
                charset: &partner.charset,
                sender: route.sender,
                recipient: route.recipient,
                reference,
                time,
                wrong_count,
            },
            m,
        ),
        Outgoing::X12(_) | Outgoing::Ta1(_) => {
            let settings = partner.x12();
            let control: u64 = reference
                .trim_start_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .with_context(|| format!("X12 control number {reference:?} is not numeric"))?;
            let version = match message {
                Outgoing::X12(m) => m.version.clone().unwrap_or(settings.version.clone()),
                _ => settings.version.clone(),
            };
            let env = x12::Envelope {
                delimiters: settings.delimiters()?,
                sender: party(route.sender, route.applications.0),
                receiver: party(route.recipient, route.applications.1),
                control,
                group: control,
                version: &version,
                usage: settings.usage,
                ack_requested: false,
                time,
                line_ending: b"",
                wrong_count,
            };
            match message {
                Outgoing::X12(m) => x12::envelope(
                    &env,
                    &[x12::Outgoing {
                        kind: &m.kind,
                        reference: "0001",
                        implementation: m.implementation.as_deref(),
                        body: &m.body,
                    }],
                ),
                Outgoing::Ta1(segment) => x12::ta1_interchange(&env, segment),
                Outgoing::Edifact(_) => bail!("unreachable"),
            }
        }
    }
}

/// What a script would do with an interchange: its log lines, the messages
/// it would send, and whether it asked for an acknowledgement. Nothing is
/// stored or sent; partner state starts empty.
pub struct Trial {
    pub logs: Vec<String>,
    pub messages: Vec<String>,
    pub acknowledge: bool,
}

pub fn try_script(
    standard: Standard,
    raw: &[u8],
    source: &str,
    function: &str,
    now: i64,
) -> Result<Trial> {
    let parsed = parse(raw);
    let first = parsed
        .interchanges
        .first()
        .context("the message is not an interchange Vendredi can read")?;
    let effects = crate::script::run(
        source,
        function,
        &crate::script::Input {
            interchange: &first.view(),
            legacy: first.legacy(),
            state: &Default::default(),
            reference: 0,
            now,
        },
    )?;
    let messages = effects
        .sends
        .iter()
        .enumerate()
        .map(|(n, send)| {
            let message = match send {
                Value::String(raw) if standard == Standard::Edifact => {
                    edi::script_document(raw).map(Outgoing::Edifact)
                }
                Value::String(_) => bail!("send a message from api.build to an X12 partner"),
                other => contract::built(other, standard),
            };
            message
                .and_then(|m| m.text())
                .with_context(|| format!("script send {}", n + 1))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Trial {
        logs: effects.logs,
        messages,
        acknowledge: effects.acknowledge,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn scripts_can_be_tried_on_a_message() -> Result<()> {
        let order = include_bytes!("../../../examples/orders/order.edi");
        let source = "function reply(i, api) { const m = i.groups[0].messages[0]; api.log('got ' + m.type); api.send(api.build('ORDRSP', m.version, [['BGM', '231', 'X', '29']])); api.ack(); }";
        let trial = try_script(Standard::Edifact, order, source, "reply", 0)?;
        assert_eq!(trial.logs, ["got ORDERS"]);
        assert_eq!(trial.messages.len(), 1);
        assert!(
            trial.messages[0].contains("BGM+231+X+29'"),
            "{:?}",
            trial.messages
        );
        assert!(trial.acknowledge);
        assert!(try_script(Standard::Edifact, b"nonsense", source, "reply", 0).is_err());
        Ok(())
    }

    use super::*;

    #[test]
    fn detects_and_summarizes_both_standards() {
        let edifact = parse(include_bytes!("../../../examples/orders/order.edi"));
        let x12 = parse(include_bytes!("../../../examples/x12/850-5010.edi"));
        assert_eq!(edifact.interchanges[0].standard(), Standard::Edifact);
        assert_eq!(x12.interchanges[0].standard(), Standard::X12);
        assert_eq!(edifact.interchanges[0].sender(), ["BUYER", "ZZ"]);
        assert_eq!(x12.interchanges[0].sender(), ["BUYER", "ZZ"]);
        assert_eq!(x12.interchanges[0].recipient(), ["SUPPLIER", "ZZ"]);
        let messages = x12.interchanges[0].messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(
            (
                messages[2].kind.as_str(),
                messages[2].version.as_str(),
                messages[2].group.as_str()
            ),
            ("850", "005010", "1002")
        );
        assert_eq!(edifact.interchanges[0].messages()[0].version, "D96A");
        assert_eq!(detect(b"garbage"), None);
        assert!(!parse(b"garbage").findings.is_empty());
    }
}
