//! Plain explanations of EDI segment tags, shown when hovering a segment.

/// What a segment is for, for the tags most messages use.
pub(super) fn segment(tag: &str) -> Option<&'static str> {
    Some(match tag {
        // EDIFACT service segments.
        "UNA" => "Service string advice: the separators this interchange uses",
        "UNB" => "Interchange header: who sends, who receives, when, and the interchange reference",
        "UNG" => "Functional group header",
        "UNH" => "Message header: the message type, its version and the message reference",
        "UNT" => "Message trailer: the segment count and the message reference again",
        "UNE" => "Functional group trailer",
        "UNZ" => "Interchange trailer: the message count and the interchange reference again",
        "UCI" => "Acknowledgement of the whole interchange: accepted (7) or rejected (4)",
        "UCM" => "Acknowledgement of one message",
        // EDIFACT business segments.
        "BGM" => "Beginning of message: the document type and number, such as the order number",
        "DTM" => "Date or time, with a qualifier saying which one",
        "NAD" => "Name and address of a party, such as the buyer or supplier",
        "LIN" => "Line item: one product of the order",
        "QTY" => "Quantity, with a qualifier saying which one",
        "PRI" => "Price",
        "MOA" => "Monetary amount",
        "RFF" => "Reference to another document",
        "FTX" => "Free text",
        "CUX" => "Currencies",
        "TAX" => "Tax details",
        "PIA" => "Additional product identification",
        "IMD" => "Item description",
        "CNT" => "Control total",
        // X12 envelope and acknowledgements.
        "ISA" => {
            "Interchange header: sender, receiver, date, control number and test or production use"
        }
        "IEA" => "Interchange trailer: the group count and the control number again",
        "GS" => "Functional group header: application codes, version and group control number",
        "GE" => "Functional group trailer: the transaction count and group control number",
        "ST" => "Transaction set header: the transaction type, such as 850, and its control number",
        "SE" => "Transaction set trailer: the segment count and control number",
        "TA1" => "Interchange acknowledgement",
        "AK1" => "Acknowledged functional group",
        "AK2" => "Acknowledged transaction set",
        "AK5" => "Transaction set result: accepted (A) or rejected (R)",
        "AK9" => "Functional group result and counts",
        "IK5" => "Transaction set result in a 999",
        // X12 business segments.
        "BEG" => "Beginning of a purchase order: its purpose, type and number",
        "BAK" => "Beginning of a purchase order acknowledgement",
        "PO1" => "Purchase order line item",
        "N1" => "Party name, such as ship-to or bill-to",
        "REF" => "Reference identification",
        "CTT" => "Transaction totals",
        _ => return None,
    })
}

/// The tag and text of each segment in an EDIFACT or X12 document, split at
/// segment terminators that are not escaped.
pub(super) fn segments(text: &str) -> Vec<(&str, &str)> {
    let x12 = text.trim_start().starts_with("ISA") || text.trim_start().starts_with("ST*");
    let (terminator, element) = if x12 { ('~', '*') } else { ('\'', '+') };
    let mut out = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if !x12 && c == '?' && !escaped {
            escaped = true;
            continue;
        }
        if c == terminator && !escaped {
            let raw = text[start..=i].trim();
            if !raw.is_empty() {
                let tag = raw.split([element, terminator]).next().unwrap_or_default();
                out.push((tag, raw));
            }
            start = i + c.len_utf8();
        }
        escaped = false;
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn common_segments_are_explained() {
        for tag in ["UNB", "UNH", "BGM", "UCI", "ISA", "ST", "AK5", "BEG"] {
            assert!(super::segment(tag).is_some(), "{tag}");
        }
        assert!(super::segment("ZZZ").is_none());
        let edifact = super::segments("UNH+1+ORDRSP:D:96A:UN'\nFTX+AAI+++a?'b'UNT+3+1'");
        assert_eq!(
            edifact.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
            ["UNH", "FTX", "UNT"]
        );
        assert_eq!(edifact[1].1, "FTX+AAI+++a?'b'");
        let x12 = super::segments("ST*855*0001~BAK*00*AC*PO1~SE*3*0001~");
        assert_eq!(
            x12.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
            ["ST", "BAK", "SE"]
        );
    }
}
