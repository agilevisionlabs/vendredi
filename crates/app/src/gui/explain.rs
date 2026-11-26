//! Common failures in plain language, with what to do next. The full
//! error chain stays available as details.
use std::io::ErrorKind;

/// A sentence for the reader and the technical chain behind it.
pub(super) struct Explained {
    pub text: String,
    pub details: String,
}

pub(super) fn explain(error: &anyhow::Error) -> Explained {
    let details = format!("{error:#}");
    let io = error
        .chain()
        .find_map(|e| e.downcast_ref::<std::io::Error>())
        .map(std::io::Error::kind);
    let text = if io == Some(ErrorKind::AddrInUse) || details.contains("Address already in use") {
        "Another app is using the listening port. Pause it there, or change the listen address in Settings.".to_owned()
    } else if io == Some(ErrorKind::PermissionDenied) && details.contains("bind") {
        "Vendredi may not listen on this port. Choose a port above 1024 in Settings.".to_owned()
    } else if details.contains("workspace is already in use") {
        "This workspace is open in another Vendredi window or command. Close it there, then try again.".to_owned()
    } else if details.contains("rules now live in cases") {
        "A partner still lists rules in vendredi.toml. Move them into a case.".to_owned()
    } else if let Some(case) = details
        .split("case ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .filter(|_| details.contains("is not installed"))
    {
        format!("The case {case} is not installed. Install or import it, or choose another case.")
    } else if io == Some(ErrorKind::NotFound) {
        match missing_path(&details) {
            Some(path) => format!("{path} is missing. Restore the file, or choose another one."),
            None => "A file Vendredi needs is missing.".to_owned(),
        }
    } else if details.contains("SyntaxError") {
        "A script has a syntax error. Open it on the Scripts page to fix it.".to_owned()
    } else if details.contains("TOML parse error")
        || details.contains("expected") && details.contains("vendredi.toml")
    {
        "vendredi.toml has a mistake. Open Settings → Edit configuration file to fix it.".to_owned()
    } else {
        // Unknown failures keep their first line, which is usually the clearest.
        error.to_string()
    };
    Explained { text, details }
}

/// The first path-like word in an error chain, such as `cases/orders/reply.edi`.
fn missing_path(details: &str) -> Option<&str> {
    // Error chains join their parts with ": ". A lone colon may belong to a
    // Windows drive, as in C:\Users, so only ": " separates parts.
    details
        .split(": ")
        .flat_map(|part| part.split(' '))
        .map(|w| w.trim_matches(|c: char| c == '"' || c == '\''))
        .find(|w| {
            w.contains(['/', '\\'])
                || w.ends_with(".edi")
                || w.ends_with(".js")
                || w.ends_with(".json")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn common_failures_read_as_sentences() {
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let error = std::net::TcpListener::bind(busy.local_addr().unwrap())
            .context("binding 127.0.0.1")
            .unwrap_err();
        assert!(
            explain(&error)
                .text
                .contains("Another app is using the listening port")
        );
        let missing = std::fs::read("/nonexistent/cases/orders/reply.edi")
            .context("/nonexistent/cases/orders/reply.edi")
            .unwrap_err();
        let explained = explain(&missing);
        assert_eq!(
            explained.text,
            "/nonexistent/cases/orders/reply.edi is missing. Restore the file, or choose another one."
        );
        // The system's own wording follows in the details.
        assert!(explained.details.len() > explained.text.len() / 2);
        assert_eq!(
            missing_path(
                r"C:\Users\me\cases\reply.edi: The system cannot find the path specified."
            ),
            Some(r"C:\Users\me\cases\reply.edi")
        );
        let locked = anyhow::anyhow!("workspace is already in use");
        assert!(
            explain(&locked)
                .text
                .contains("open in another Vendredi window")
        );
        let other = anyhow::anyhow!("something odd").context("Doing a thing");
        assert_eq!(explain(&other).text, "Doing a thing");
    }
}
