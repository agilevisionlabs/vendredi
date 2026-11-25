//! The versioned, standard-neutral data that scripts and cases are written against.
//!
//! Scripts never see the internal EDI types. They receive the view built here
//! and return messages in the same shape, so the EDI layer can change without
//! breaking the scripts in a workspace or a shared case library.
use crate::{
    config::Standard,
    edi::{Interchange, Message, Segment},
};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

/// Increment when the shape of the script view changes incompatibly.
pub const FORMAT: u32 = 1;

fn element(components: &[String]) -> Value {
    match components {
        [single] => json!(single),
        _ => json!(components),
    }
}

/// A segment is an array: the tag, then one item per element. A simple
/// element is a string and a composite element is an array of components.
pub fn segment(s: &Segment) -> Value {
    let mut items = vec![json!(s.tag)];
    items.extend(s.elements.iter().map(|e| element(e)));
    Value::Array(items)
}

/// The message as scripts see it. Header and trailer segments are omitted,
/// because the engine writes them for every message it sends.
pub fn message(m: &Message) -> Value {
    let h = m.header();
    let body = &m.segments[1..m.segments.len().saturating_sub(1).max(1)];
    let mut v = json!({
        "type": m.kind(),
        "version": m.version(),
        "reference": m.reference(),
        "segments": body.iter().map(segment).collect::<Vec<_>>(),
    });
    for (key, index) in [("agency", 3), ("association", 4)] {
        if !h.at(1, index).is_empty() {
            v[key] = json!(h.at(1, index));
        }
    }
    v
}

/// The inbound interchange as scripts see it. EDIFACT interchanges carry one
/// implicit group, because Vendredi does not support `UNG` groups.
pub fn interchange(i: &Interchange, standard: Standard) -> Value {
    let h = &i.header;
    json!({
        "format": FORMAT,
        "standard": standard,
        "sender": h.elements.get(1).cloned().unwrap_or_default(),
        "recipient": h.elements.get(2).cloned().unwrap_or_default(),
        "reference": h.at(4, 0),
        "test": h.at(10, 0) == "1",
        "groups": [{ "messages": i.messages.iter().map(message).collect::<Vec<_>>() }],
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Built {
    #[serde(rename = "type")]
    kind: String,
    version: String,
    segments: Vec<Vec<Value>>,
    #[serde(default)]
    agency: Option<String>,
    #[serde(default)]
    association: Option<String>,
    /// The X12 `ST03` implementation convention reference.
    #[serde(default)]
    implementation: Option<String>,
    /// Present when a script sends an inbound message object back. The engine
    /// always assigns its own reference.
    #[serde(default)]
    #[allow(dead_code)]
    reference: Option<String>,
}

fn text(value: &Value, what: &str) -> Result<String> {
    let s = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        _ => bail!("{what} must be a string or number"),
    };
    ensure!(
        s.chars().all(|c| u32::from(c) <= 0xff),
        "{what} contains a character outside Latin-1"
    );
    Ok(s)
}

const ENVELOPE_TAGS: [&str; 7] = ["UNA", "UNB", "UNG", "UNH", "UNT", "UNE", "UNZ"];

/// Converts a message a script built with `api.build` into the EDI model.
pub fn built(value: &Value, standard: Standard) -> Result<crate::interchange::Outgoing> {
    use crate::interchange::{Outgoing, X12Message};
    let b: Built = serde_json::from_value(value.clone()).context("invalid message object")?;
    if standard == Standard::X12 {
        ensure!(
            b.agency.is_none() && b.association.is_none(),
            "agency and association are EDIFACT settings"
        );
        ensure!(
            !b.kind.is_empty() && b.kind.bytes().all(|c| c.is_ascii_digit()),
            "expected an X12 transaction set such as 855, found {:?}",
            b.kind
        );
        ensure!(
            b.version.len() >= 6 && b.version[..6].bytes().all(|c| c.is_ascii_digit()),
            "expected an X12 version such as 005010, found {:?}",
            b.version
        );
        let body = b
            .segments
            .iter()
            .enumerate()
            .map(|(n, items)| {
                let s = crate::x12::segment_from_view(items)
                    .with_context(|| format!("segment {}", n + 1))?;
                ensure!(
                    !matches!(s.tag.as_str(), "ISA" | "IEA" | "GS" | "GE" | "ST" | "SE"),
                    "segment {} is an envelope segment ({}). The engine writes envelopes",
                    n + 1,
                    s.tag
                );
                Ok(s)
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(Outgoing::X12(X12Message {
            kind: b.kind,
            version: Some(b.version),
            implementation: b.implementation,
            body,
        }));
    }
    edifact(b).map(Outgoing::Edifact)
}

fn edifact(b: Built) -> Result<Message> {
    ensure!(
        !b.kind.is_empty() && b.kind.bytes().all(|c| c.is_ascii_alphanumeric()),
        "invalid message type {:?}",
        b.kind
    );
    let (directory, release) = match b.version.split_once(':') {
        Some((d, r)) => (d, r),
        None if b.version.is_char_boundary(1) => b.version.split_at(1),
        None => bail!("invalid version {:?}", b.version),
    };
    ensure!(
        !directory.is_empty()
            && !release.is_empty()
            && b.version
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b':'),
        "expected an EDIFACT version such as D96A, found {:?}",
        b.version
    );
    let mut identifier = vec![
        b.kind,
        directory.into(),
        release.into(),
        b.agency.unwrap_or_else(|| "UN".into()),
    ];
    identifier.extend(b.association);
    let mut segments = vec![Segment::new("UNH", vec![vec!["1".into()], identifier])];
    for (n, items) in b.segments.iter().enumerate() {
        let what = format!("segment {}", n + 1);
        let (tag, elements) = items
            .split_first()
            .with_context(|| format!("{what} is empty"))?;
        let tag = text(tag, &what)?;
        ensure!(
            tag.len() == 3 && tag.bytes().all(|c| c.is_ascii_uppercase()),
            "{what} has an invalid tag {tag:?}"
        );
        ensure!(
            !ENVELOPE_TAGS.contains(&tag.as_str()),
            "{what} is an envelope segment ({tag}). The engine writes envelopes"
        );
        let elements = elements
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let what = format!("{tag} element {}", i + 1);
                match e {
                    Value::Array(components) => components
                        .iter()
                        .map(|c| text(c, &what))
                        .collect::<Result<Vec<_>>>(),
                    other => Ok(vec![text(other, &what)?]),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        segments.push(Segment::new(&tag, elements));
    }
    segments.push(Segment::new("UNT", Vec::new()));
    Ok(Message { segments })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edi;

    #[test]
    fn view_round_trips_through_built_messages() -> Result<()> {
        let raw = b"UNA:+.? 'UNB+UNOC:3+BUYER:ZZ+SUPPLIER:ZZ+261124:1030+42++++++1'UNH+7+ORDERS:D:96A:UN:EAN008'BGM+220+PO?+42+9'DTM+137:20261124:102'UNT+4+7'UNZ+1+42'";
        let parsed = edi::parse(raw);
        let i = &parsed.interchanges[0];
        assert!(i.findings.is_empty(), "{:?}", i.findings);
        let view = interchange(i, Standard::Edifact);
        assert_eq!(view["sender"], json!(["BUYER", "ZZ"]));
        assert_eq!(view["test"], true);
        let m = &view["groups"][0]["messages"][0];
        assert_eq!(m["type"], "ORDERS");
        assert_eq!(m["version"], "D96A");
        assert_eq!(m["reference"], "7");
        assert_eq!(m["association"], "EAN008");
        assert_eq!(
            m["segments"],
            json!([
                ["BGM", "220", "PO+42", "9"],
                ["DTM", ["137", "20261124", "102"]]
            ])
        );
        let crate::interchange::Outgoing::Edifact(rebuilt) = built(m, Standard::Edifact)? else {
            panic!("expected EDIFACT");
        };
        for (a, b) in rebuilt.segments[1..3]
            .iter()
            .zip(&i.messages[0].segments[1..3])
        {
            assert_eq!((&a.tag, &a.elements), (&b.tag, &b.elements));
        }
        assert_eq!(
            rebuilt.header().elements[1],
            i.messages[0].header().elements[1]
        );
        Ok(())
    }

    #[test]
    fn built_messages_reject_envelopes_and_bad_values() {
        let bad = [
            json!({"type":"ORDRSP","version":"D96A","segments":[["UNB","x"]]}),
            json!({"type":"ORDRSP","version":"D96A","segments":[["bgm","x"]]}),
            json!({"type":"ORDRSP","version":"D96A","segments":[["BGM",["a",["b"]]]]}),
            json!({"type":"ORDRSP","version":"D96A","segments":[["FTX","€"]]}),
            json!({"type":"ORDRSP","version":"D","segments":[]}),
            json!({"type":"ORDRSP","segments":[]}),
            json!({"type":"ORD RSP","version":"D96A","segments":[]}),
            json!({"type":"ORDRSP","version":"D96A","segments":[[]]}),
        ];
        for v in bad {
            assert!(built(&v, Standard::Edifact).is_err(), "{v}");
        }
        let crate::interchange::Outgoing::Edifact(m) = built(
            &json!({"type":"ORDRSP","version":"D:01B","segments":[["QTY",["21",5]],["FTX",null,"Café"]]}),
            Standard::Edifact,
        )
        .unwrap() else {
            panic!("expected EDIFACT");
        };
        assert_eq!(m.version(), "D01B");
        assert_eq!(m.segments[1].elements, [vec!["21", "5"]]);
        assert_eq!(m.segments[2].at(1, 0), "Café");
    }
}
